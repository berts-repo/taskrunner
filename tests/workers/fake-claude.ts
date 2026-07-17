import { chmodSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tempDir } from "../helpers.js";

/**
 * Writes an executable node script that mimics `claude --print
 * --output-format stream-json --verbose` closely enough for harness tests:
 * the line shapes observed live (system init with session_id, assistant
 * tool_use edits, a final result line), resume support, and failure/hang
 * modes driven by the prompt text.
 */
export function writeFakeClaude(): string {
  const script = `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");

const args = process.argv.slice(2);
const prompt = args[args.length - 1] ?? "";
const resumeIndex = args.indexOf("--resume");
const sessionId = resumeIndex >= 0 ? args[resumeIndex + 1] : "sess-" + process.pid;
const isResume = resumeIndex >= 0;

const emit = (obj) => process.stdout.write(JSON.stringify(obj) + "\\n");

if (prompt.includes("exit-nonzero")) {
  process.stderr.write("fake claude blew up\\n");
  process.exit(3);
}

emit({ type: "system", subtype: "init", session_id: sessionId, tools: ["Write", "Edit"] });

if (prompt.includes("hang")) {
  setInterval(() => {}, 1000);
} else if (prompt.includes("result-error")) {
  emit({ type: "result", subtype: "error_during_execution", is_error: true, result: "fake claude task failed", session_id: sessionId });
} else {
  const file = path.join(process.cwd(), "hello.txt");
  fs.appendFileSync(file, isResume ? "line two\\n" : "line one\\n");
  emit({
    type: "assistant",
    session_id: sessionId,
    message: {
      content: [
        { type: "tool_use", name: "Write", input: { file_path: file } },
        { type: "text", text: "writing hello.txt" },
      ],
    },
  });
  emit({ type: "user", session_id: sessionId, message: { content: [{ type: "tool_result", content: "ok" }] } });
  emit({
    type: "result",
    subtype: "success",
    is_error: false,
    result: (isResume ? "resumed " : "started ") + sessionId + " for: " + prompt,
    usage: { input_tokens: 7, output_tokens: 3 },
    session_id: sessionId,
  });
}
`;
  const dir = tempDir("fake-claude");
  const bin = join(dir, "fake-claude.cjs");
  writeFileSync(bin, script);
  chmodSync(bin, 0o755);
  return bin;
}
