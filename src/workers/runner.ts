import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createInterface } from "node:readline";
import type { Readable } from "node:stream";
import { ToolError } from "../domain/errors.js";

// Per-turn execution runtime for worker processes. Harnesses build a worker
// argv (codex/claude CLI invocations) and the runner runs it inside a Docker
// container with the workspace mounted at /workspace behind the egress proxy
// (PLAN § Workspace And Isolation). `kind: "host"` exists only for the local
// test runner that exercises harnesses without Docker.

export interface WorkerSpawnSpec {
  /** Logical worker argv, e.g. ["codex", "exec", ...]. */
  argv: string[];
  env?: Record<string, string>;
}

export interface RunningWorker {
  stdout: Readable;
  stderr: Readable;
  /** Resolves with the exit code (null when killed by signal). */
  exited: Promise<number | null>;
  /** Terminates the worker; used on cancel and timeout. */
  kill(): void;
}

export interface WorkerRunner {
  readonly kind: "host" | "docker";
  /** Workspace path as the worker process sees it. */
  readonly workspacePath: string;
  start(spec: WorkerSpawnSpec): Promise<RunningWorker>;
  /** Post-turn cleanup; the scheduler calls this exactly once. */
  dispose(): Promise<void>;
}

function wrapChild(child: ChildProcess, kill: () => void): RunningWorker {
  return {
    stdout: child.stdout as Readable,
    stderr: child.stderr as Readable,
    exited: new Promise((resolve, reject) => {
      child.on("error", reject);
      child.on("close", (code) => resolve(code));
    }),
    kill,
  };
}

export interface EgressDecision {
  allowed: boolean;
  host: string;
  port: number;
}

export interface DockerRunnerOptions {
  /** Host path of the task workspace (a task-local clone). */
  workspaceDir: string;
  /** Uniquifies container/network names; the turn id. */
  scopeId: string;
  image: string;
  /** Docker volume with the worker's own login, e.g. taskrunner-codex-home. */
  authVolume?: string;
  /** Container mount point for the auth volume. */
  authMount?: string;
  proxyImage: string;
  /** Egress allowlist for this turn: worker defaults plus approved additions. */
  allowedDomains: string[];
  onEgress?: (decision: EgressDecision) => void;
  dockerCommand?: string;
}

const PROXY_PORT = 3128;

function docker(
  command: string,
  args: string[],
): { ok: boolean; stdout: string; stderr: string } {
  const result = spawnSync(command, args, { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
  return {
    ok: result.status === 0,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
  };
}

/**
 * Runs the worker in a container on an internal Docker network (no route to
 * the outside). A dual-homed egress proxy sidecar is the only way out and
 * forwards only allowlisted domains; every decision it makes is surfaced via
 * onEgress for the audit log (PLAN § Container posture).
 */
export class DockerRunner implements WorkerRunner {
  readonly kind = "docker";
  readonly workspacePath = "/workspace";

  private readonly networkName: string;
  private readonly proxyName: string;
  private readonly workerName: string;
  private readonly dockerCmd: string;
  private proxyStarted = false;
  private networkCreated = false;
  private logsChild: ChildProcess | undefined;

  constructor(private readonly options: DockerRunnerOptions) {
    this.networkName = `taskrunner-egress-${options.scopeId}`;
    this.proxyName = `taskrunner-proxy-${options.scopeId}`;
    this.workerName = `taskrunner-worker-${options.scopeId}`;
    this.dockerCmd = options.dockerCommand ?? "docker";
  }

  async start(spec: WorkerSpawnSpec): Promise<RunningWorker> {
    this.preflight();
    const proxyIp = await this.startProxy();

    const proxyUrl = `http://${proxyIp}:${PROXY_PORT}`;
    const args = [
      "run",
      "-i",
      "--rm",
      "--name",
      this.workerName,
      "--network",
      this.networkName,
      "-v",
      `${this.options.workspaceDir}:/workspace`,
      "-w",
      "/workspace",
      "-e",
      `HTTP_PROXY=${proxyUrl}`,
      "-e",
      `HTTPS_PROXY=${proxyUrl}`,
      "-e",
      `http_proxy=${proxyUrl}`,
      "-e",
      `https_proxy=${proxyUrl}`,
      "-e",
      "NO_PROXY=localhost,127.0.0.1",
    ];
    if (this.options.authVolume) {
      args.push("-v", `${this.options.authVolume}:${this.options.authMount ?? "/home/worker"}`);
    }
    for (const [key, value] of Object.entries(spec.env ?? {})) {
      args.push("-e", `${key}=${value}`);
    }
    args.push(this.options.image, ...spec.argv);

    const child = spawn(this.dockerCmd, args, { stdio: ["ignore", "pipe", "pipe"] });
    const kill = () => {
      // The docker CLI proxies signals, but killing the container by name is
      // the reliable path when the attach stream is wedged.
      docker(this.dockerCmd, ["kill", this.workerName]);
      child.kill("SIGTERM");
    };
    return wrapChild(child, kill);
  }

  /** Checks docker, image, and auth volume upfront for clear errors. */
  private preflight(): void {
    if (!this.options.image) {
      throw new ToolError(
        "not_configured",
        "this worker has no Docker image configured; set [worker.<name>] image",
      );
    }
    const info = docker(this.dockerCmd, ["version", "--format", "{{.Server.Version}}"]);
    if (!info.ok) {
      throw new ToolError(
        "worker_unavailable",
        "Docker is not available; start Docker Desktop",
      );
    }
    if (!docker(this.dockerCmd, ["image", "inspect", this.options.image]).ok) {
      throw new ToolError(
        "not_configured",
        `worker image '${this.options.image}' is not built; run: npm run build:images`,
      );
    }
    if (!docker(this.dockerCmd, ["image", "inspect", this.options.proxyImage]).ok) {
      throw new ToolError(
        "not_configured",
        `egress proxy image '${this.options.proxyImage}' is not built; run: npm run build:images`,
      );
    }
    if (
      this.options.authVolume &&
      !docker(this.dockerCmd, ["volume", "inspect", this.options.authVolume]).ok
    ) {
      throw new ToolError(
        "not_configured",
        `worker auth volume '${this.options.authVolume}' does not exist; ` +
          `create it and log the worker in (see README § Worker sign-in)`,
      );
    }
  }

  private async startProxy(): Promise<string> {
    const created = docker(this.dockerCmd, [
      "network",
      "create",
      "--internal",
      this.networkName,
    ]);
    if (!created.ok) {
      throw new ToolError(
        "internal_error",
        `failed to create egress network: ${created.stderr.trim()}`,
      );
    }
    this.networkCreated = true;

    const run = docker(this.dockerCmd, [
      "run",
      "-d",
      "--name",
      this.proxyName,
      "--network",
      this.networkName,
      "-e",
      `TASKRUNNER_ALLOWED_DOMAINS=${JSON.stringify(this.options.allowedDomains)}`,
      this.options.proxyImage,
    ]);
    if (!run.ok) {
      throw new ToolError(
        "internal_error",
        `failed to start egress proxy: ${run.stderr.trim()}`,
      );
    }
    this.proxyStarted = true;

    // Second leg: the proxy (and only the proxy) can reach the outside.
    const connected = docker(this.dockerCmd, ["network", "connect", "bridge", this.proxyName]);
    if (!connected.ok) {
      throw new ToolError(
        "internal_error",
        `failed to connect egress proxy to the outside network: ${connected.stderr.trim()}`,
      );
    }

    const inspected = docker(this.dockerCmd, [
      "inspect",
      "-f",
      `{{(index .NetworkSettings.Networks "${this.networkName}").IPAddress}}`,
      this.proxyName,
    ]);
    const proxyIp = inspected.stdout.trim();
    if (!inspected.ok || !proxyIp) {
      throw new ToolError("internal_error", "could not determine egress proxy address");
    }

    await this.watchProxyLogs();
    return proxyIp;
  }

  /** Streams proxy decisions; resolves once the proxy reports it is listening. */
  private watchProxyLogs(): Promise<void> {
    this.logsChild = spawn(this.dockerCmd, ["logs", "-f", this.proxyName], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    const rl = createInterface({ input: this.logsChild.stdout as Readable });
    return new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new ToolError("internal_error", "egress proxy did not start in time")),
        10_000,
      );
      rl.on("line", (line) => {
        let obj: Record<string, unknown>;
        try {
          obj = JSON.parse(line) as Record<string, unknown>;
        } catch {
          return;
        }
        if (obj["proxy"] === "listening") {
          clearTimeout(timer);
          resolve();
          return;
        }
        if (typeof obj["egress"] === "string") {
          this.options.onEgress?.({
            allowed: obj["egress"] === "allowed",
            host: String(obj["host"] ?? ""),
            port: Number(obj["port"] ?? 0),
          });
        }
      });
      this.logsChild?.on("close", () => clearTimeout(timer));
    });
  }

  async dispose(): Promise<void> {
    // Give the log stream a beat to flush trailing egress decisions.
    if (this.logsChild) {
      await new Promise((resolve) => setTimeout(resolve, 200));
      this.logsChild.kill("SIGTERM");
    }
    docker(this.dockerCmd, ["rm", "-f", this.workerName]);
    if (this.proxyStarted) docker(this.dockerCmd, ["rm", "-f", this.proxyName]);
    if (this.networkCreated) docker(this.dockerCmd, ["network", "rm", this.networkName]);
  }
}
