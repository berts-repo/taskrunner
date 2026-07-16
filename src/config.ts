import * as fs from "node:fs";
import { parse } from "smol-toml";
import { z } from "zod";

// Global TOML config at <state root>/config.toml. Keys are user-facing and
// tracked in docs/specs/NAMING.md; keep this schema minimal.

const workerSchema = z.object({
  /** Host-runtime command override (e.g. a codex binary path). */
  command: z.string().optional(),
  /** Where worker processes run; host is privileged and needs approval. */
  runtime: z.enum(["docker", "host"]).default("docker"),
  image: z.string().optional(),
  auth_volume: z.string().optional(),
  /** Egress allowlist defaults: the worker's own API domains. */
  allowed_domains: z.array(z.string()).default([]),
});

export type WorkerConfig = z.infer<typeof workerSchema>;

const configSchema = z.object({
  daemon: z.object({}).passthrough().optional(),
  task: z
    .object({
      turn_timeout_seconds: z.number().int().positive().default(1800),
    })
    .default({}),
  worker: z
    .object({
      codex: workerSchema
        .extend({
          image: z.string().default("taskrunner/codex-worker"),
          auth_volume: z.string().default("taskrunner-codex-home"),
          allowed_domains: z
            .array(z.string())
            .default(["api.openai.com", "auth.openai.com", "chatgpt.com", "*.chatgpt.com"]),
        })
        .default({}),
      claude: workerSchema
        .extend({
          image: z.string().default("taskrunner/claude-worker"),
          auth_volume: z.string().default("taskrunner-claude-home"),
          allowed_domains: z
            .array(z.string())
            .default(["api.anthropic.com", "*.anthropic.com", "claude.ai"]),
        })
        .default({}),
    })
    .default({}),
  egress: z
    .object({
      proxy_image: z.string().default("taskrunner/egress-proxy"),
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

/** Worker settings with schema defaults for workers not named in the schema. */
export function workerConfig(config: Config, worker: string): WorkerConfig {
  const known = config.worker as Record<string, WorkerConfig | undefined>;
  return known[worker] ?? workerSchema.parse({});
}
