// Applies each event from a log to a fresh index, reporting the first event
// that fails. Usage: tsx scripts/debug-refold.ts <events.jsonl> [out.db]
// With out.db the index is written to disk (the Rust port's parity check
// diffs it against its own fold of the same log; see scripts/parity-index.sh).
import { readEvents } from "../src/storage/events.js";
import { StateIndex } from "../src/storage/index.js";

const path = process.argv[2]!;
const index = new StateIndex(process.argv[3] ?? ":memory:");
for (const event of readEvents(path)) {
  try {
    index.apply(event);
  } catch (err) {
    console.log(`FAILS at ${event.id} (${event.type}): ${String(err)}`);
    console.log(JSON.stringify(event, null, 2));
    process.exit(1);
  }
}
index.close();
console.log("refold clean");
