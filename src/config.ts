import * as fs from "node:fs";
import { parse } from "smol-toml";
import { z } from "zod";

// Global TOML config at <state root>/config.toml. Keys are user-facing and
// tracked in docs/specs/NAMING.md; keep this schema minimal.
const configSchema = z.object({
  daemon: z.object({}).passthrough().optional(),
  task: z
    .object({
      turn_timeout_seconds: z.number().int().positive().default(1800),
    })
    .default({}),
  worker: z
    .object({
      codex: z.object({ command: z.string().default("codex") }).default({}),
    })
    .default({}),
});

export type Config = z.infer<typeof configSchema>;

export function parseConfig(raw: unknown): Config {
  return configSchema.parse(raw);
}

export function loadConfig(configFile: string): Config {
  let raw: unknown = {};
  try {
    raw = parse(fs.readFileSync(configFile, "utf8"));
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code !== "ENOENT") throw err;
  }
  return configSchema.parse(raw);
}
