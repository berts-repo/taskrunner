/**
 * End-to-end verification of the built dist CLI per the approved plan:
 * shim auto-start, assign/lookup/continue/cancel, conflict, worktree branch,
 * kill -9 crash recovery, and index delete-and-rebuild. The codex worker
 * command is overridden in config.toml with the scripted stand-in binary and
 * runs with runtime = "host", which also exercises the Phase 2 privileged
 * approval choreography: every assign waits for `taskrunner approve`.
 */
import { execFileSync, spawnSync } from "node:child_process";
import * as fs from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { writeFakeCodex } from "../tests/workers/fake-codex.js";

const REPO_ROOT = join(import.meta.dirname, "..");
const CLI = join(REPO_ROOT, "dist", "cli.js");
// Keep this short: the state root must yield a unix socket path under the
// ~104-byte sun_path limit.
const SCRATCH = process.env["TASKRUNNER_VERIFY_DIR"] ?? fs.mkdtempSync(join(tmpdir(), "trv-"));
const STATE = join(SCRATCH, "s");

let failures = 0;
function check(name: string, ok: boolean, detail = ""): void {
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
  if (!ok) failures++;
}

function newClient(): { client: Client; transport: StdioClientTransport } {
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [CLI, "mcp", "--state-root", STATE],
    stderr: "pipe",
  });
  const client = new Client({ name: "verify-e2e", version: "0" });
  return { client, transport };
}

function text(result: any): string {
  return (result.content as { text: string }[]).map((c) => c.text).join("\n");
}

async function main(): Promise<void> {
  console.log(`state root: ${STATE}`);
  fs.rmSync(STATE, { recursive: true, force: true });
  fs.mkdirSync(STATE, { recursive: true });
  const fakeCodex = writeFakeCodex();
  fs.writeFileSync(
    join(STATE, "config.toml"),
    `[task]\nturn_timeout_seconds = 120\n\n[worker.codex]\ncommand = "${fakeCodex}"\nruntime = "host"\n`,
  );

  const repo = join(SCRATCH, "verify-repo");
  fs.rmSync(repo, { recursive: true, force: true });
  fs.mkdirSync(repo);
  const git = (cwd: string, ...args: string[]) =>
    execFileSync("git", ["-C", cwd, ...args], { encoding: "utf8" });
  git(repo, "init", "-q");
  git(repo, "config", "user.email", "v@example.com");
  git(repo, "config", "user.name", "V");
  fs.writeFileSync(join(repo, "README.md"), "verify\n");
  git(repo, "add", ".");
  git(repo, "commit", "-qm", "init");

  // 1. Shim auto-starts the daemon.
  const { client, transport } = newClient();
  await client.connect(transport);
  check("shim auto-starts daemon (pid file exists)", fs.existsSync(join(STATE, "runtime/daemon.pid")));

  const approve = (id: string) =>
    spawnSync(process.execPath, [CLI, "approve", id, "--state-root", STATE], {
      encoding: "utf8",
    });

  // 2. assign-task on a host-runtime (privileged) worker waits for approval.
  const assign = await client.callTool({
    name: "assign-task",
    arguments: { project: repo, worker: "codex", prompt: "create hello" },
  });
  const assignText = text(assign);
  const taskId = /task: (task_\w+)/.exec(assignText)?.[1] ?? "";
  check(
    "host-runtime assign waits for human approval",
    assignText.includes("tier: privileged") &&
      assignText.includes("approval: pending") &&
      assignText.includes(`taskrunner approve ${taskId}`),
    taskId,
  );
  const early = await client.callTool({
    name: "continue-task",
    arguments: { taskId: taskId, prompt: "too soon" },
  });
  check(
    "continue-task before approval returns approval_required",
    text(early).startsWith("error approval_required:"),
  );

  // 2b. taskrunner deny blocks a task permanently.
  const denyAssign = await client.callTool({
    name: "assign-task",
    arguments: { project: repo, worker: "codex", prompt: "should never run" },
  });
  const denyId = /task: (task_\w+)/.exec(text(denyAssign))?.[1] ?? "";
  const denyOut = spawnSync(process.execPath, [CLI, "deny", denyId, "--state-root", STATE], {
    encoding: "utf8",
  });
  const deniedCont = await client.callTool({
    name: "continue-task",
    arguments: { taskId: denyId, prompt: "please?" },
  });
  check(
    "taskrunner deny blocks the task from running",
    denyOut.status === 0 && text(deniedCont).startsWith("error policy_denied:"),
    denyOut.stdout.trim() || denyOut.stderr.trim(),
  );

  // 2c. taskrunner approve starts the waiting first turn.
  const approveOut = approve(taskId);
  check(
    "taskrunner approve starts the waiting turn",
    approveOut.status === 0 && approveOut.stdout.includes("approved"),
    approveOut.stdout.trim() || approveOut.stderr.trim(),
  );

  // 3. lookup until completed.
  let status = "";
  for (let i = 0; i < 100; i++) {
    const lookup = await client.callTool({ name: "lookup-task", arguments: { taskId: taskId } });
    status = /status: (\w+)/.exec(text(lookup))?.[1] ?? "";
    if (status !== "running") break;
    await sleep(100);
  }
  check("turn completes via lookup-task polling", status === "completed", `status=${status}`);

  // 4. Worktree exists on the taskrunner/<task_id> branch with the edit.
  const workspace = join(STATE, "workspaces", taskId);
  const branch = git(workspace, "branch", "--show-current").trim();
  check("task workspace on taskrunner/<task_id> branch", branch === `taskrunner/${taskId}`, branch);
  check(
    "worker edit landed in workspace, not project",
    fs.existsSync(join(workspace, "hello.txt")) && !fs.existsSync(join(repo, "hello.txt")),
  );

  // 5. continue-task resumes the same native session.
  const cont = await client.callTool({
    name: "continue-task",
    arguments: { taskId: taskId, prompt: "extend hello", wait: true },
  });
  check("continue-task resumes same thread", /resumed thread-/.test(text(cont)));

  // 6. Trace replays both turns.
  const trace = await client.callTool({
    name: "lookup-task",
    arguments: { taskId: taskId, include: ["trace"] },
  });
  const traceText = text(trace);
  check(
    "trace replays both turns end-to-end",
    traceText.includes("trace: turn 1") &&
      traceText.includes("trace: turn 2") &&
      traceText.includes("worker.file_change"),
  );

  // 7. Conflict while a turn runs, then cancel.
  const hang = await client.callTool({
    name: "assign-task",
    arguments: { project: repo, worker: "codex", prompt: "hang around" },
  });
  const hangId = /task: (task_\w+)/.exec(text(hang))?.[1] ?? "";
  approve(hangId); // starts the hanging turn
  const conflict = await client.callTool({
    name: "continue-task",
    arguments: { taskId: hangId, prompt: "more" },
  });
  check("continue-task during running turn returns conflict", text(conflict).startsWith("error conflict:"));
  const cancel = await client.callTool({
    name: "cancel-task",
    arguments: { taskId: hangId, reason: "verification cleanup" },
  });
  check("cancel-task lands canceled with audit", text(cancel).includes("status: canceled"));

  // 8. kill -9 mid-turn, then recovery marks the turn failed.
  const hang2 = await client.callTool({
    name: "assign-task",
    arguments: { project: repo, worker: "codex", prompt: "hang two" },
  });
  const hang2Id = /task: (task_\w+)/.exec(text(hang2))?.[1] ?? "";
  approve(hang2Id); // starts the hanging turn
  const daemonPid = Number(fs.readFileSync(join(STATE, "runtime/daemon.pid"), "utf8"));
  process.kill(daemonPid, "SIGKILL");
  await sleep(300);
  await client.close().catch(() => {});

  const second = newClient();
  await second.client.connect(second.transport); // auto-starts a fresh daemon past the stale lock
  const recovered = await second.client.callTool({
    name: "lookup-task",
    arguments: { taskId: hang2Id },
  });
  check(
    "kill -9 recovery: interrupted turn recorded worker_failed",
    text(recovered).includes("status: failed"),
    /status: (\w+)/.exec(text(recovered))?.[1],
  );
  const summaryBefore = text(
    await second.client.callTool({
      name: "lookup-task",
      arguments: { taskId: taskId, include: ["turns"] },
    }),
  );
  await second.client.close().catch(() => {});

  // 9. Delete index.db; rebuild from events.jsonl gives identical lookup.
  spawnSync(process.execPath, [CLI, "down", "--state-root", STATE], { encoding: "utf8" });
  fs.rmSync(join(STATE, "index.db"), { force: true });
  fs.rmSync(join(STATE, "index.db-wal"), { force: true });
  fs.rmSync(join(STATE, "index.db-shm"), { force: true });
  const third = newClient();
  await third.client.connect(third.transport);
  const summaryAfter = text(
    await third.client.callTool({
      name: "lookup-task",
      arguments: { taskId: taskId, include: ["turns"] },
    }),
  );
  check("index delete-and-rebuild yields identical lookup", summaryAfter === summaryBefore);
  if (summaryAfter !== summaryBefore) {
    const before = summaryBefore.split("\n");
    const after = summaryAfter.split("\n");
    for (let i = 0; i < Math.max(before.length, after.length); i++) {
      if (before[i] !== after[i]) {
        console.log(`  first difference at line ${i}:`);
        console.log(`  before: ${before[i]}`);
        console.log(`  after:  ${after[i]}`);
        break;
      }
    }
  }
  await third.client.close().catch(() => {});

  spawnSync(process.execPath, [CLI, "down", "--state-root", STATE], { encoding: "utf8" });
  console.log(failures === 0 ? "\nALL CHECKS PASSED" : `\n${failures} CHECKS FAILED`);
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((err) => {
  console.error("verification crashed:", err);
  process.exit(1);
});
