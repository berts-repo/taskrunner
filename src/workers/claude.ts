import { createInterface } from "node:readline";
import type { TurnRequest, TurnResult, WorkerHarness } from "./harness.js";

// Claude Code worker harness over the live-verified control surface
// (PLAN § Worker Harness):
//   start:  claude --print --output-format stream-json --verbose <perm> <prompt>
//   resume: same, plus --resume <session_id>
// Non-interactive runs MUST set a permission flag or they hang on permission
// prompts. In Docker the container plus egress proxy are the boundary, so
// Claude's own permission prompts are disabled; on the host, acceptEdits
// allows file edits but nothing broader.
//
// Claude emits no dedicated file-change event; changed files are derived
// from tool_use inputs (Write/Edit/MultiEdit/NotebookEdit), with the
// workspace git fallback covering anything shell commands touched.

interface ClaudeLine {
  [key: string]: unknown;
}

const EDIT_TOOLS = new Set(["Write", "Edit", "MultiEdit", "NotebookEdit"]);

function extractEditedFiles(message: ClaudeLine, workspacePath: string, into: Set<string>): void {
  const content = message["content"];
  if (!Array.isArray(content)) return;
  for (const block of content) {
    const b = block as ClaudeLine;
    if (b["type"] !== "tool_use" || !EDIT_TOOLS.has(String(b["name"]))) continue;
    const input = (b["input"] ?? {}) as ClaudeLine;
    const path = input["file_path"] ?? input["notebook_path"];
    if (typeof path !== "string") continue;
    const prefix = workspacePath.endsWith("/") ? workspacePath : `${workspacePath}/`;
    into.add(path.startsWith(prefix) ? path.slice(prefix.length) : path);
  }
}

export interface ClaudeHarnessOptions {
  /** Model to request, passed as `--model`. */
  model?: string;
}

export class ClaudeHarness implements WorkerHarness {
  readonly name = "claude";

  constructor(private readonly options: ClaudeHarnessOptions = {}) {}

  async runTurn(request: TurnRequest): Promise<TurnResult> {
    const args = ["claude", "--print", "--output-format", "stream-json", "--verbose"];
    if (this.options.model) args.push("--model", this.options.model);
    if (request.runner.kind === "docker") {
      args.push("--dangerously-skip-permissions");
    } else {
      args.push("--permission-mode", "acceptEdits");
    }
    if (request.nativeSessionId) args.push("--resume", request.nativeSessionId);
    args.push(request.prompt);

    const worker = await request.runner.start({ argv: args });

    return new Promise<TurnResult>((resolve, reject) => {
      let sessionId: string | undefined = request.nativeSessionId;
      let response = "";
      let usage: unknown;
      let resultError: string | undefined;
      const changedFiles = new Set<string>();
      let stderrTail = "";
      let settled = false;

      const onAbort = () => worker.kill();
      request.signal.addEventListener("abort", onAbort, { once: true });

      const rl = createInterface({ input: worker.stdout });
      rl.on("line", (line) => {
        if (line.trim() === "") return;
        let obj: ClaudeLine;
        try {
          obj = JSON.parse(line) as ClaudeLine;
        } catch {
          request.onEvent({ kind: "unparsed_output", payload: { line } });
          return;
        }
        // Event kinds quote Claude's own line types verbatim
        // (system, assistant, user, result, rate_limit_event, ...).
        const kind = typeof obj["type"] === "string" ? (obj["type"] as string) : "unknown";
        request.onEvent({ kind, payload: obj });

        if (typeof obj["session_id"] === "string") sessionId = obj["session_id"] as string;
        if (kind === "assistant" && obj["message"]) {
          extractEditedFiles(
            obj["message"] as ClaudeLine,
            request.runner.workspacePath,
            changedFiles,
          );
        }
        if (kind === "result") {
          if (typeof obj["result"] === "string") response = obj["result"] as string;
          if (obj["usage"] !== undefined) usage = obj["usage"];
          if (obj["is_error"] === true) {
            resultError = typeof obj["result"] === "string"
              ? (obj["result"] as string)
              : `claude reported ${String(obj["subtype"] ?? "an error")}`;
          }
        }
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
            settle(() => reject(new Error("claude worker terminated by abort")));
            return;
          }
          if (code !== 0 || resultError) {
            const detail = resultError ?? stderrTail.trim();
            settle(() =>
              reject(
                new Error(`claude exited with code ${code}${detail ? `: ${detail}` : ""}`),
              ),
            );
            return;
          }
          settle(() =>
            resolve({
              response,
              ...(sessionId ? { nativeSessionId: sessionId } : {}),
              changedFiles: [...changedFiles],
              ...(usage !== undefined ? { usage } : {}),
              exitCode: code,
            }),
          );
        })
        .catch((err: Error) =>
          settle(() => reject(new Error(`failed to start claude worker: ${err.message}`))),
        );
    });
  }
}
