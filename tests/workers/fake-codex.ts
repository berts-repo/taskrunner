import { chmodSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tempDir } from "../helpers.js";

/**
 * Writes an executable node script that mimics `codex exec --json` closely
 * enough for harness tests: JSONL events on stdout in the shapes recorded in
 * docs/specs/BACKEND_SPIKE.md, a real file edit in the workspace, resume
 * support, and failure/hang modes driven by the prompt text.
 */
export function writeFakeCodex(): string {
  const script = `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");

const args = process.argv.slice(2);
const isResume = args.includes("resume");
const prompt = args[args.length - 1] ?? "";
const cIndex = args.indexOf("-C");
const workspace = cIndex >= 0 ? args[cIndex + 1] : process.cwd();
const threadId = isResume ? args[args.indexOf("resume") + 2] : "thread-" + process.pid;

const emit = (obj) => process.stdout.write(JSON.stringify(obj) + "\\n");

if (prompt.includes("exit-nonzero")) {
  process.stderr.write("fake codex blew up\\n");
  process.exit(3);
}

emit({ type: "thread.started", thread_id: threadId });
emit({ type: "turn.started" });

if (prompt.includes("hang")) {
  // Stay alive until killed.
  setInterval(() => {}, 1000);
} else {
  const file = path.join(workspace, "hello.txt");
  fs.appendFileSync(file, isResume ? "line two\\n" : "line one\\n");
  emit({ type: "item.completed", item: { item_type: "command_execution", command: "append hello.txt" } });
  emit({ type: "item.completed", item: { item_type: "file_change", changes: [{ path: "hello.txt" }] } });
  emit({
    type: "item.completed",
    item: { item_type: "agent_message", text: (isResume ? "resumed " : "started ") + threadId + " for: " + prompt },
  });
  emit({ type: "turn.completed", usage: { input_tokens: 10, output_tokens: 5 } });
}
`;
  const dir = tempDir("fake-codex");
  const bin = join(dir, "fake-codex.cjs");
  writeFileSync(bin, script);
  chmodSync(bin, 0o755);
  return bin;
}
