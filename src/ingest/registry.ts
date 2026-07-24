import { claudeCodeParser } from "./claude-code.js";
import { codexParser } from "./codex.js";
import type { TranscriptParser } from "./parser.js";

// The single table that knows which transcript formats have parser code.
// Adding a new source (e.g. Hermes/OpenClaw) is a config entry naming its
// `format` plus one parser registered here — never a sweeper change.
export const PARSERS: Record<string, TranscriptParser> = {
  "claude-code": claudeCodeParser,
  codex: codexParser,
};

export function parserForFormat(format: string): TranscriptParser | undefined {
  return PARSERS[format];
}
