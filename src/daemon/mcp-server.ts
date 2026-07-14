import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { Config } from "../config.js";
import type { StatePaths } from "../paths.js";
import type { ArtifactStore } from "../storage/artifacts.js";
import type { EventBody, LogEvent } from "../storage/events.js";
import type { StateIndex } from "../storage/index.js";
import { VERSION } from "../version.js";

/** Everything a tool handler needs from the daemon. */
export interface ToolContext {
  paths: StatePaths;
  config: Config;
  index: StateIndex;
  artifacts: ArtifactStore;
  /** Appends to the event log and folds into the index — the only write path. */
  record: (body: EventBody) => LogEvent;
  /** Durable Taskrunner session for the connected client. */
  sessionId: string;
}

export function createMcpServer(_ctx: ToolContext): McpServer {
  const server = new McpServer(
    { name: "taskrunner", version: VERSION },
    { capabilities: {} },
  );
  // The four core tools (assign-task, continue-task, lookup-task, cancel-task)
  // are registered here in M3.
  return server;
}
