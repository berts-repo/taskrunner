#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");

const args = process.argv.slice(2);
const isResume = args.includes("resume");
const prompt = args[args.length - 1] ?? "";
const cIndex = args.indexOf("-C");
const workspace = cIndex >= 0 ? args[cIndex + 1] : process.cwd();
const threadId = isResume ? args[args.indexOf("resume") + 2] : "thread-" + process.pid;

const emit = (obj) => process.stdout.write(JSON.stringify(obj) + "\n");

if (prompt.includes("exit-nonzero")) {
  process.stderr.write("fake codex blew up\n");
  process.exit(3);
}

emit({ type: "thread.started", thread_id: threadId });
emit({ type: "turn.started" });

if (prompt.includes("exit-leaving-stderr-open")) {
  // Exits, but a child it started keeps stderr open, so reading stderr to its
  // end would wait on the child instead of the worker.
  require("node:child_process")
    .spawn("sleep", ["30"], { stdio: ["ignore", "ignore", "inherit"], detached: true })
    .unref();
  emit({ type: "item.completed", item: { item_type: "agent_message", text: "done" } });
  emit({ type: "turn.completed", usage: { input_tokens: 1, output_tokens: 1 } });
  process.exit(0);
}

if (prompt.includes("close-stdout-and-hang")) {
  // Stops writing but keeps running: reading its output ends on its own,
  // which is where cancellation used to stop being watched. Closing fd 1 is
  // what actually ends the pipe — process.stdout.end() leaves it open.
  fs.closeSync(1);
  setInterval(() => {}, 1000);
} else if (prompt.includes("hang")) {
  // Stay alive until killed.
  setInterval(() => {}, 1000);
} else {
  const file = path.join(workspace, "hello.txt");
  fs.appendFileSync(file, isResume ? "line two\n" : "line one\n");
  emit({ type: "item.completed", item: { item_type: "command_execution", command: "append hello.txt" } });
  emit({ type: "item.completed", item: { item_type: "file_change", changes: [{ path: "hello.txt" }] } });
  emit({
    type: "item.completed",
    item: { item_type: "agent_message", text: (isResume ? "resumed " : "started ") + threadId + " for: " + prompt },
  });
  emit({ type: "turn.completed", usage: { input_tokens: 10, output_tokens: 5 } });
}
