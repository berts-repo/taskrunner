import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import type { TurnRequest, TurnResult, WorkerHarness } from "./harness.js";

// Codex worker harness over the spike-proven control surface
// (docs/specs/BACKEND_SPIKE.md):
//   start:  codex -a never -s workspace-write exec --json -C <workspace> <prompt>
//   resume: codex -a never -s workspace-write exec resume --json <thread_id> <prompt>
// Events are one JSON object per stdout line. The parser is deliberately
// tolerant of shape differences across codex versions: every line becomes an
// audit event, and only thread id, agent messages, file changes, and usage
// are interpreted.

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

export class CodexHarness implements WorkerHarness {
  readonly name = "codex";

  constructor(private readonly command: string = "codex") {}

  runTurn(request: TurnRequest): Promise<TurnResult> {
    const args = ["-a", "never", "-s", "workspace-write", "exec"];
    if (request.nativeSessionId) {
      args.push("resume", "--json", request.nativeSessionId, request.prompt);
    } else {
      args.push("--json", "-C", request.workspaceDir, request.prompt);
    }

    return new Promise<TurnResult>((resolve, reject) => {
      const child = spawn(this.command, args, {
        cwd: request.workspaceDir,
        stdio: ["ignore", "pipe", "pipe"],
      });

      let threadId: string | undefined = request.nativeSessionId;
      let lastAgentMessage: string | undefined;
      let usage: unknown;
      const changedFiles = new Set<string>();
      let stderrTail = "";
      let settled = false;

      const onAbort = () => {
        child.kill("SIGTERM");
        const hardKill = setTimeout(() => child.kill("SIGKILL"), 2000);
        hardKill.unref();
      };
      request.signal.addEventListener("abort", onAbort, { once: true });

      const rl = createInterface({ input: child.stdout });
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

      child.stderr.on("data", (chunk: Buffer) => {
        stderrTail = (stderrTail + chunk.toString("utf8")).slice(-4096);
      });

      const settle = (fn: () => void) => {
        if (settled) return;
        settled = true;
        request.signal.removeEventListener("abort", onAbort);
        fn();
      };

      child.on("error", (err) =>
        settle(() => reject(new Error(`failed to start codex worker: ${err.message}`))),
      );
      child.on("close", (code) => {
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
            changedFiles: [...changedFiles],
            ...(usage !== undefined ? { usage } : {}),
            exitCode: code,
          }),
        );
      });
    });
  }
}
