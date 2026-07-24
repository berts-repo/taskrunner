import { spawnSync } from "node:child_process";
import * as fs from "node:fs";
import { Agent, fetch as undiciFetch } from "undici";
import { loadConfig, workerConfig, type Config } from "./config.js";
import { AUTH_MOUNTS, DEFAULT_IMAGES, ingestSources } from "./daemon/daemon.js";
import { expandHome } from "./ingest/sweep.js";
import type { StatePaths } from "./paths.js";
import { VERSION } from "./version.js";

// `taskrunner doctor`: a read-only preflight over the pieces a delegated turn
// needs — Docker, worker images, auth volumes, the egress proxy image — plus
// ingestion health and best-effort worker-credential freshness. It reuses the
// same config-driven worker enumeration the daemon uses, so it can never
// drift from what actually runs. Nothing here mutates state.

type Level = "ok" | "warn" | "fail";

interface Check {
  level: Level;
  label: string;
  detail: string;
}

const MARK: Record<Level, string> = { ok: "✓", warn: "!", fail: "✗" };

/** Runs a docker subcommand, capturing output; never throws. */
function docker(args: string[]): { ok: boolean; stdout: string; stderr: string } {
  const r = spawnSync("docker", args, {
    encoding: "utf8",
    timeout: 15_000,
    maxBuffer: 4 * 1024 * 1024,
  });
  return { ok: r.status === 0, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
}

async function checkDaemon(paths: StatePaths, checks: Check[]): Promise<void> {
  const agent = new Agent({ connect: { socketPath: paths.socketPath } });
  try {
    const res = await undiciFetch("http://taskrunner/status", {
      dispatcher: agent,
      signal: AbortSignal.timeout(2000),
    });
    const body = (await res.json()) as { version: string };
    if (body.version === VERSION) {
      checks.push({ level: "ok", label: "daemon", detail: `running, version ${body.version}` });
    } else {
      checks.push({
        level: "warn",
        label: "daemon",
        detail: `running version ${body.version}, but this CLI is ${VERSION}; restart the daemon to update`,
      });
    }
  } catch {
    checks.push({
      level: "warn",
      label: "daemon",
      detail: "not running (it auto-starts on the next MCP connection)",
    });
  } finally {
    await agent.close();
  }
}

function checkDocker(checks: Check[]): boolean {
  const info = docker(["version", "--format", "{{.Server.Version}}"]);
  if (info.ok) {
    checks.push({ level: "ok", label: "docker", detail: `engine ${info.stdout.trim()}` });
    return true;
  }
  checks.push({
    level: "fail",
    label: "docker",
    detail: "not available; start Docker Desktop (workers and hub cannot run without it)",
  });
  return false;
}

/** Worker names exactly as the daemon enumerates them. */
function workerNames(config: Config): string[] {
  return [...new Set(["codex", "claude", ...Object.keys(config.worker)])];
}

function checkWorkers(config: Config, dockerUp: boolean, checks: Check[]): void {
  const seenImages = new Set<string>();
  for (const name of workerNames(config)) {
    const cfg = workerConfig(config, name);
    const kind = cfg.harness ?? name;
    const image = cfg.image ?? DEFAULT_IMAGES[kind] ?? "";
    if (!image) {
      checks.push({ level: "fail", label: `worker ${name}`, detail: "no image configured" });
      continue;
    }
    if (dockerUp) {
      const has = docker(["image", "inspect", image]).ok;
      checks.push({
        level: has ? "ok" : "fail",
        label: `worker ${name} image`,
        detail: has ? image : `${image} not built; run: npm run build:images`,
      });
      seenImages.add(image);
    }
    if (cfg.auth_volume && dockerUp) {
      const has = docker(["volume", "inspect", cfg.auth_volume]).ok;
      checks.push({
        level: has ? "ok" : "fail",
        label: `worker ${name} auth`,
        detail: has
          ? cfg.auth_volume
          : `auth volume '${cfg.auth_volume}' missing; log the worker in (see README § Worker sign-in)`,
      });
      if (has) checkCredential(name, kind, cfg.auth_volume, image, checks);
    }
  }
  if (dockerUp) {
    const proxy = config.egress.proxy_image;
    const has = docker(["image", "inspect", proxy]).ok;
    checks.push({
      level: has ? "ok" : "fail",
      label: "egress proxy",
      detail: has ? proxy : `${proxy} not built; run: npm run build:images`,
    });
  }
}

/** Where each harness kind keeps its credential file inside its auth volume. */
const CREDENTIAL_FILE: Record<string, string> = {
  codex: "auth.json",
  claude: ".claude/.credentials.json",
};

/** Best-effort: report credential presence and any parseable expiry. Never
 * downgrades to fail — a stale read here should not block anything. */
function checkCredential(
  name: string,
  kind: string,
  volume: string,
  image: string,
  checks: Check[],
): void {
  const rel = CREDENTIAL_FILE[kind];
  if (!rel) return;
  const read = docker(["run", "--rm", "-v", `${volume}:/v:ro`, image, "cat", `/v/${rel}`]);
  if (!read.ok) {
    checks.push({
      level: "warn",
      label: `worker ${name} credential`,
      detail: `no ${rel} in the auth volume; the worker may need to log in`,
    });
    return;
  }
  const expiry = parseExpiry(read.stdout);
  if (expiry === undefined) {
    checks.push({ level: "ok", label: `worker ${name} credential`, detail: "present" });
    return;
  }
  const expired = expiry <= Date.now();
  checks.push({
    level: expired ? "warn" : "ok",
    label: `worker ${name} credential`,
    detail: expired
      ? `expired ${new Date(expiry).toISOString()}; re-run the worker login`
      : `valid until ${new Date(expiry).toISOString()}`,
  });
}

/** Pulls a millisecond expiry out of known credential shapes, if present. */
function parseExpiry(text: string): number | undefined {
  let json: unknown;
  try {
    json = JSON.parse(text);
  } catch {
    return undefined;
  }
  const oauth = (json as { claudeAiOauth?: { expiresAt?: unknown } }).claudeAiOauth;
  if (oauth && typeof oauth.expiresAt === "number") return oauth.expiresAt;
  const tokens = (json as { tokens?: { expires_at?: unknown } }).tokens;
  if (tokens && typeof tokens.expires_at === "string") {
    const ms = Date.parse(tokens.expires_at);
    if (!Number.isNaN(ms)) return ms;
  }
  return undefined;
}

function checkIngest(config: Config, paths: StatePaths, checks: Check[]): void {
  for (const source of ingestSources(config)) {
    if (source.volume) {
      // Volume sources are copied out via Docker at sweep time; presence is
      // covered by the worker auth-volume check above, so just note the route.
      checks.push({
        level: "ok",
        label: `ingest ${source.format} (volume)`,
        detail: `${source.volume}/${source.subdir ?? ""}`,
      });
      continue;
    }
    const dirs = source.dirs ?? [];
    const present = dirs.map(expandHome).filter((dir) => fs.existsSync(dir));
    checks.push({
      level: present.length > 0 ? "ok" : "warn",
      label: `ingest ${source.format}`,
      detail:
        present.length > 0
          ? `${present.length}/${dirs.length} source dir(s) present`
          : `no source dirs found (${dirs.join(", ")}); nothing to archive yet`,
    });
  }
  if (fs.existsSync(paths.ingestStateFile)) {
    try {
      JSON.parse(fs.readFileSync(paths.ingestStateFile, "utf8"));
      checks.push({ level: "ok", label: "ingest state", detail: "sidecar parseable" });
    } catch {
      checks.push({
        level: "warn",
        label: "ingest state",
        detail: "sidecar unparseable; it will be rebuilt on the next sweep (harmless)",
      });
    }
  }
}

export async function runDoctor(paths: StatePaths): Promise<number> {
  const config = loadConfig(paths.configFile);
  const checks: Check[] = [];
  await checkDaemon(paths, checks);
  const dockerUp = checkDocker(checks);
  checkWorkers(config, dockerUp, checks);
  checkIngest(config, paths, checks);

  for (const c of checks) {
    process.stdout.write(`  ${MARK[c.level]} ${c.label}: ${c.detail}\n`);
  }
  const failures = checks.filter((c) => c.level === "fail").length;
  const warnings = checks.filter((c) => c.level === "warn").length;
  process.stdout.write(
    `\ntaskrunner doctor: ${failures} failing, ${warnings} warning(s), ` +
      `${checks.length - failures - warnings} ok\n`,
  );
  return failures > 0 ? 1 : 0;
}
