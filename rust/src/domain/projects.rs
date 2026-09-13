//! Resolving a caller-supplied path to a project record.

use std::fs;
use std::path::Path;

use rusqlite::OptionalExtension;

use super::errors::ToolError;
use crate::ids::{IdPrefix, new_id};
use crate::storage::Recorder;
use crate::storage::events::EventBody;
use crate::storage::index::StateIndex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRef {
    pub project_id: String,
    pub root: String,
}

/// Resolves a caller-supplied path to a canonical project record, creating the
/// project on first sight. Projects are keyed by normalized (real) root path;
/// other observed paths become aliases.
pub fn resolve_project(
    index: &StateIndex,
    recorder: &dyn Recorder,
    project_path: &str,
) -> Result<ProjectRef, ToolError> {
    if !Path::new(project_path).is_absolute() {
        return Err(ToolError::invalid_request(format!(
            "project path must be absolute: {project_path}"
        )));
    }
    let root = fs::canonicalize(project_path).map_err(|_| {
        ToolError::invalid_request(format!("project path does not exist: {project_path}"))
    })?;
    if !root.is_dir() {
        return Err(ToolError::invalid_request(format!(
            "project path is not a directory: {project_path}"
        )));
    }
    let root = root.to_string_lossy().into_owned();

    let by_alias: Option<(String, String)> = index
        .db
        .query_row(
            "SELECT p.id, p.root FROM project_aliases a JOIN projects p ON p.id = a.project_id
             WHERE a.path = ?",
            [&root],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((project_id, root_of_record)) = by_alias {
        if project_path != root {
            record_alias(index, recorder, &project_id, project_path)?;
        }
        return Ok(ProjectRef { project_id, root: root_of_record });
    }

    let project_id = new_id(IdPrefix::Project);
    recorder
        .record(EventBody::ProjectCreated { project_id: project_id.clone(), root: root.clone() })?;
    if project_path != root {
        record_alias(index, recorder, &project_id, project_path)?;
    }
    Ok(ProjectRef { project_id, root })
}

fn record_alias(
    index: &StateIndex,
    recorder: &dyn Recorder,
    project_id: &str,
    path: &str,
) -> Result<(), ToolError> {
    let existing: Option<String> = index
        .db
        .query_row("SELECT project_id FROM project_aliases WHERE path = ?", [path], |row| {
            row.get(0)
        })
        .optional()?;
    if existing.is_none() {
        recorder.record(EventBody::ProjectAliasAdded {
            project_id: project_id.to_string(),
            path: path.to_string(),
        })?;
    }
    Ok(())
}
