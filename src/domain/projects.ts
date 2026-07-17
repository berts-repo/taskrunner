import * as fs from "node:fs";
import { isAbsolute } from "node:path";
import { newId } from "../ids.js";
import type { EventBody, LogEvent } from "../storage/events.js";
import type { StateIndex } from "../storage/index.js";
import { ToolError } from "./errors.js";

export interface ProjectRef {
  project_id: string;
  root: string;
}

/**
 * Resolves a caller-supplied path to a canonical project record, creating the
 * project on first sight. Projects are keyed by normalized (real) root path;
 * other observed paths become aliases.
 */
export function resolveProject(
  index: StateIndex,
  record: (body: EventBody) => LogEvent,
  projectPath: string,
): ProjectRef {
  if (!isAbsolute(projectPath)) {
    throw new ToolError("invalid_request", `project path must be absolute: ${projectPath}`);
  }
  let root: string;
  try {
    root = fs.realpathSync(projectPath);
  } catch {
    throw new ToolError("invalid_request", `project path does not exist: ${projectPath}`);
  }
  if (!fs.statSync(root).isDirectory()) {
    throw new ToolError("invalid_request", `project path is not a directory: ${projectPath}`);
  }

  const byAlias = index.db
    .prepare(
      `SELECT p.id, p.root FROM project_aliases a JOIN projects p ON p.id = a.project_id
       WHERE a.path = ?`,
    )
    .get(root) as { id: string; root: string } | undefined;
  if (byAlias) {
    if (projectPath !== root) recordAlias(index, record, byAlias.id, projectPath);
    return { project_id: byAlias.id, root: byAlias.root };
  }

  const projectId = newId("proj");
  record({ type: "project.created", project_id: projectId, root });
  if (projectPath !== root) recordAlias(index, record, projectId, projectPath);
  return { project_id: projectId, root };
}

function recordAlias(
  index: StateIndex,
  record: (body: EventBody) => LogEvent,
  projectId: string,
  path: string,
): void {
  const existing = index.db
    .prepare("SELECT project_id FROM project_aliases WHERE path = ?")
    .get(path) as { project_id: string } | undefined;
  if (!existing) record({ type: "project.alias-added", project_id: projectId, path });
}
