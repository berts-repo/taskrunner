import * as fs from "node:fs";
import { parse } from "smol-toml";
import { z } from "zod";

// Global TOML config at <state root>/config.toml. Keys are user-facing;
// keep this schema minimal.

const workerSchema = z.object({
  /**
   * Which harness code drives this worker. Defaults to the worker's own
   * name, so `[worker.codex]` needs nothing; a custom worker names the loop
   * it reuses (e.g. `[worker.qwen] harness = "codex"`).
   */
  harness: z.enum(["codex", "claude"]).optional(),
  /** Model the harness asks for (e.g. "gpt-oss:20b", "qwen2.5-coder:32b"). */
  model: z.string().optional(),
  /** Local model server type; setting it puts the codex harness in --oss mode. */
  provider: z.enum(["ollama", "lmstudio"]).optional(),
  image: z.string().optional(),
  auth_volume: z.string().optional(),
  /** Egress allowlist defaults: the worker's own API domains. */
  allowed_domains: z.array(z.string()).default([]),
});

export type WorkerConfig = z.infer<typeof workerSchema>;

// Transcript ingestion sources. Like workers, sources are pluggable: a
// source is an [ingest.sources.<name>] entry whose `format` selects the
// parser code (the built-ins claude-code/codex default their format to
// their own name). Interval and sources live under one [ingest] section;
// sources nest one level deeper so the scalar interval_seconds does not
// collide with the source catchall.
// A source here is always a set of *host* directories. Worker transcripts
// living inside Docker auth volumes are not configured: they are derived from
// each worker's own `auth_volume` and `image` (see ingestSources), so the
// volume name, the image used to reach into it, and the per-harness subdir
// can never drift out of agreement with the worker they belong to.
//
// Strict on purpose: an unknown key here — a stray `volume`, a `dir` typo —
// would otherwise be dropped in silence, and a source that silently ingests
// nothing is worse than one that refuses to load.
const ingestSourceSchema = z
  .object({
    /** Parser format; a custom source names the parser it reuses. */
    format: z.string(),
    /** Host directories to scan (recursively) for this source's transcripts. */
    dirs: z.array(z.string()).default([]),
  })
  .strict();

export type IngestSourceConfig = z.infer<typeof ingestSourceSchema>;

const configSchema = z.object({
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
            // platform.claude.com serves the OAuth token refresh; blocking it
            // strands the worker with 401s once its access token ages out.
            .default(["api.anthropic.com", "*.anthropic.com", "claude.ai", "platform.claude.com"]),
        })
        .default({}),
    })
    // Workers are pluggable: any other `[worker.<name>]` section is parsed
    // with the base schema and needs only a `harness` key to come alive.
    .catchall(workerSchema)
    .default({}),
  egress: z
    .object({
      proxy_image: z.string().default("taskrunner/egress-proxy"),
    })
    .default({}),
  ingest: z
    .object({
      /** How often the daemon sweeps transcript sources into the event log. */
      interval_seconds: z.number().int().positive().default(300),
      sources: z
        .object({
          "claude-code": ingestSourceSchema
            .extend({
              format: z.string().default("claude-code"),
              dirs: z.array(z.string()).default(["~/.claude/projects"]),
            })
            .default({}),
          codex: ingestSourceSchema
            .extend({
              format: z.string().default("codex"),
              dirs: z.array(z.string()).default(["~/.codex/sessions"]),
            })
            .default({}),
        })
        // Any other [ingest.sources.<name>] is parsed with the base schema
        // and needs only a `format` key to come alive.
        .catchall(ingestSourceSchema)
        .default({}),
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
