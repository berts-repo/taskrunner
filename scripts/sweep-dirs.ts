// Sweeps host transcript directories into a fresh event log: the TypeScript
// half of scripts/parity-sweep.sh.
// Usage: tsx scripts/sweep-dirs.ts <out-dir> <claude-dir> <codex-dir>
import { join } from "node:path";
import { TranscriptSweeper } from "../src/ingest/sweep.js";
import { EventLog } from "../src/storage/events.js";
import { StateIndex } from "../src/storage/index.js";

const [outDir, claudeDir, codexDir] = process.argv.slice(2);
if (!outDir || !claudeDir || !codexDir) {
  throw new Error("usage: sweep-dirs.ts <out-dir> <claude-dir> <codex-dir>");
}
const log = EventLog.open(join(outDir, "events.jsonl"));
const index = new StateIndex(":memory:");
const sweeper = new TranscriptSweeper({
  sources: [
    { format: "claude-code", dirs: [claudeDir] },
    { format: "codex", dirs: [codexDir] },
  ],
  index,
  record: (body) => {
    const event = log.append(body, { sync: false });
    index.apply(event);
    return event;
  },
  flush: () => log.flush(),
  stateFile: join(outDir, "ingest-state.json"),
});
const stats = await sweeper.sweep();
console.log(`swept ${stats.filesScanned} files, ${stats.recorded} messages, ${stats.errors} errors`);
log.close();
