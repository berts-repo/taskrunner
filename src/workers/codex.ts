import { createInterface } from "node:readline";
import type { TurnRequest, TurnResult, WorkerHarness } from "./harness.js";

// Codex worker harness over the live-verified control surface
// (PLAN § Worker Harness):
//   start:  codex -a never -s <sandbox> exec --json -C <workspace> <prompt>
//   resume: codex -a never -s <sandbox> exec resume --json <thread_id> <prompt>
// On the host, codex's own workspace-write sandbox is the boundary. In
// Docker the container plus egress proxy are the boundary, and codex's
// sandbox (Landlock) is unavailable inside containers, so the CLI sandbox is
// disabled there. Events are one JSON object per stdout line; the parser is
// deliberately tolerant of shape differences across codex versions.

interface CodexLine {
  [key: string]: unknown;
}

function deriveKind(obj: CodexLine): string {
  const type = typeof obj["type"] === "string" ? (obj["type"] as string) : "unknown";
  const item = obj["item"] as CodexLine | undefined;
  if (type.startsWith("item.") && item) {
    const itemType = item["item_type"] ?? item["type"];
    if (typeof itemType === "string") return itemType;
  }
  return type;
}

function extractText(obj: CodexLine): string | undefined {
  const item = (obj["item"] as CodexLine | undefined) ?? obj;
  for (const key of ["text", "message", "content"]) {
    if (typeof item[key] === "string") return item[key] as string;
  }
  return undefined;
}

function extractChangedFiles(obj: CodexLine, into: Set<string>): void {
  const item = (obj["item"] as CodexLine | undefined) ?? obj;
  const changes = item["changes"] ?? obj["changes"];
  if (Array.isArray(changes)) {
    for (const change of changes) {
      if (typeof change === "string") into.add(change);
      else if (change && typeof (change as CodexLine)["path"] === "string") {
        into.add((change as CodexLine)["path"] as string);
      }
    }
  } else if (typeof item["path"] === "string") {
    into.add(item["path"] as string);
  }
}

/** Codex reports paths as the worker saw them; make them workspace-relative. */
function relativeToWorkspace(path: string, workspacePath: string): string {
  const prefix = workspacePath.endsWith("/") ? workspacePath : `${workspacePath}/`;
  return path.startsWith(prefix) ? path.slice(prefix.length) : path;
}

export interface CodexHarnessOptions {
  /** Model to request (cloud or local), passed as `-m`. */
  model?: string;
  /** Local model server type; presence switches codex into --oss mode. */
  provider?: "ollama" | "lmstudio";
}

export class CodexHarness implements WorkerHarness {
  readonly name = "codex";

  constructor(private readonly options: CodexHarnessOptions = {}) {}

  async runTurn(request: TurnRequest): Promise<TurnResult> {
    const sandbox = request.runner.kind === "docker" ? "danger-full-access" : "workspace-write";
    const args = ["codex", "-a", "never", "-s", sandbox];
    const env: Record<string, string> = {};
    if (this.options.provider) {
      args.push("--oss", "--local-provider", this.options.provider);
      if (request.runner.kind === "docker") {
        // Inside a container, "localhost" is the container; the model server
        // sits on the host, reached only through the egress proxy.
        const port = this.options.provider === "lmstudio" ? 1234 : 11434;
        env["CODEX_OSS_BASE_URL"] = `http://host.docker.internal:${port}/v1`;
      }
    }
    if (this.options.model) args.push("-m", this.options.model);
    args.push("exec");
    if (request.nativeSessionId) {
      args.push("resume", "--json", request.nativeSessionId, request.prompt);
    } else {
      args.push("--json", "-C", request.runner.workspacePath, request.prompt);
    }

    // ToolErrors from runner preflight (docker down, image or auth volume
    // missing) propagate with their error codes intact.
    const worker = await request.runner.start({
      argv: args,
      ...(Object.keys(env).length > 0 ? { env } : {}),
    });

    return new Promise<TurnResult>((resolve, reject) => {
      let threadId: string | undefined = request.nativeSessionId;
      let lastAgentMessage: string | undefined;
      let usage: unknown;
      const changedFiles = new Set<string>();
      let stderrTail = "";
      let settled = false;

      const onAbort = () => worker.kill();
      request.signal.addEventListener("abort", onAbort, { once: true });

      const rl = createInterface({ input: worker.stdout });
      rl.on("line", (line) => {
        if (line.trim() === "") return;
        let obj: CodexLine;
        try {
          obj = JSON.parse(line) as CodexLine;
        } catch {
          request.onEvent({ kind: "unparsed_output", payload: { line } });
          return;
        }
        const kind = deriveKind(obj);
        request.onEvent({ kind, payload: obj });

        if (typeof obj["thread_id"] === "string") threadId = obj["thread_id"] as string;
        if (kind === "agent_message") {
          const text = extractText(obj);
          if (text !== undefined) lastAgentMessage = text;
        }
        if (kind === "file_change") extractChangedFiles(obj, changedFiles);
        if (kind === "turn.completed" && obj["usage"] !== undefined) usage = obj["usage"];
      });

      worker.stderr.on("data", (chunk: Buffer) => {
        stderrTail = (stderrTail + chunk.toString("utf8")).slice(-4096);
      });

      const settle = (fn: () => void) => {
        if (settled) return;
        settled = true;
        request.signal.removeEventListener("abort", onAbort);
        fn();
      };

      worker.exited
        .then((code) => {
          rl.close();
          if (request.signal.aborted) {
            settle(() => reject(new Error("codex worker terminated by abort")));
            return;
          }
          if (code !== 0) {
            const detail = stderrTail.trim();
            settle(() =>
              reject(
                new Error(`codex exited with code ${code}${detail ? `: ${detail}` : ""}`),
              ),
            );
            return;
          }
          settle(() =>
            resolve({
              response: lastAgentMessage ?? "",
              ...(threadId ? { nativeSessionId: threadId } : {}),
              changedFiles: [...changedFiles].map((f) =>
                relativeToWorkspace(f, request.runner.workspacePath),
              ),
              ...(usage !== undefined ? { usage } : {}),
              exitCode: code,
            }),
          );
        })
        .catch((err: Error) =>
          settle(() => reject(new Error(`failed to start codex worker: ${err.message}`))),
        );
    });
  }
}
