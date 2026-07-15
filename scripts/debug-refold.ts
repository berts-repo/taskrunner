// Applies each event from a log to a fresh in-memory index, reporting the
// first event that fails. Usage: tsx scripts/debug-refold.ts <events.jsonl>
import { readEvents } from "../src/storage/events.js";
import { StateIndex } from "../src/storage/index.js";

const path = process.argv[2]!;
const index = new StateIndex(":memory:");
for (const event of readEvents(path)) {
  try {
    index.apply(event);
  } catch (err) {
    console.log(`FAILS at ${event.id} (${event.type}): ${String(err)}`);
    console.log(JSON.stringify(event, null, 2));
    process.exit(1);
  }
}
console.log("refold clean");
