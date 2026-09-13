// Renders a fixed set of views over an index: the TypeScript half of
// scripts/parity-views.sh. Usage: tsx scripts/render-views.ts <index.db>
import { lookupSession, lookupTask, searchTranscripts } from "../src/daemon/lookup.js";
import { listSessions } from "../src/domain/tasks.js";
import { ArtifactStore } from "../src/storage/artifacts.js";
import { StateIndex } from "../src/storage/index.js";

const index = new StateIndex(process.argv[2]!);
const deps = { index, artifacts: new ArtifactStore("/nonexistent") };
const out: string[] = [];
const section = (name: string, fn: () => string) => {
  out.push(`===== ${name}`);
  try { out.push(fn()); } catch (e) { out.push(`ERR ${(e as Error).message}`); }
};
section("sessions", () => lookupSession(index, { limit: 50 }));
for (const s of listSessions(index, { limit: 6 })) {
  const id = s.native_session_id, source = s.source;
  section(`outline ${id}`, () => lookupSession(index, { sessionId: id, source }));
  section(`compact ${id}`, () => lookupSession(index, { sessionId: id, source, view: "compact" }));
  section(`timeline ${id}`, () => lookupSession(index, { sessionId: id, source, view: "timeline" }));
  section(`prompt2 ${id}`, () => lookupSession(index, { sessionId: id, source, promptIdx: 2 }));
  section(`last3 ${id}`, () => lookupSession(index, { sessionId: id, source, scope: { last: 3 } }));
}
section("search rust", () => searchTranscripts(index, "rust", 30));
section("search rust recent", () => searchTranscripts(index, "rust", 30, { sort: "recent", role: "user" }));
section("search tool Edit", () => searchTranscripts(index, null, 30, { tool: "Edit" }));
section("search failed", () => searchTranscripts(index, null, 30, { failed: true }));
section("search target", () => searchTranscripts(index, null, 30, { target: "proxy", failed: false }));
section("search last sessions", () => searchTranscripts(index, "proxy", 30, { lastSessions: 5 }));
const tasks = index.db.prepare("SELECT id FROM tasks ORDER BY id").all() as { id: string }[];
for (const t of tasks) {
  section(`task ${t.id}`, () => lookupTask(deps, { taskId: t.id, include: ["turns", "trace", "audit", "artifacts", "transcript"] }));
  section(`task tl ${t.id}`, () => lookupTask(deps, { taskId: t.id, include: ["transcript"], view: "timeline", toolLines: 5 }));
}
process.stdout.write(out.join("\n") + "\n");
