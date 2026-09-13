// Prints a loaded config as JSON: the TypeScript half of
// scripts/parity-config.sh. Usage: tsx scripts/print-config.ts <config.toml>
import { loadConfig } from "../src/config.js";

try {
  console.log(JSON.stringify(loadConfig(process.argv[2]!), null, 2));
} catch {
  console.log("ERROR");
}
