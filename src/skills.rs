//! Agent skills, in the agentskills.io format. Taskrunner's own are built into
//! the binary so a skill always describes the tools of the version that serves
//! it. One source, two ways out: served over MCP (SEP-2640) to a harness that
//! asks for them, and written under `skills/<host>/` in the state root for a
//! harness that can't ask yet (see `sync`).
//!
//! The user's own skills, from `[skills] dirs`, are found here and linked by
//! sync, never served: a harness must treat an MCP-served skill as untrusted
//! and ask before running its scripts, which a skill the user wrote and linked
//! doesn't need.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::Delegation;

/// The capability key a server declares to serve skills (SEP-2640).
pub const EXTENSION: &str = "io.modelcontextprotocol/skills";

/// Directory name and SKILL.md of every built-in skill.
const SOURCES: [(&str, &str); 4] = [
    ("archive-search", include_str!("../skills/archive-search/SKILL.md")),
    ("delegate-task", include_str!("../skills/delegate-task/SKILL.md")),
    ("setup-harness", include_str!("../skills/setup-harness/SKILL.md")),
    ("worker-login", include_str!("../skills/worker-login/SKILL.md")),
];

/// Replaced by the sentence saying when delegation applies. It sits in the
/// description because that one line is what every harness shows the model
/// to decide whether a skill fits.
const DELEGATION_PLACEHOLDER: &str = "{delegation}";

fn delegation_sentence(mode: Delegation) -> &'static str {
    match mode {
        Delegation::Suggest => {
            "Use when the user asks to delegate or hand work off, and offer it (then wait for a \
             yes) when a task would gain from another model, a long isolated change, or \
             parallel work."
        }
        Delegation::OnRequest => {
            "Use only when the user explicitly asks to delegate or hand work to a worker, and \
             never offer it unprompted."
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The rendered SKILL.md: exactly the bytes served and written.
    pub text: String,
}

impl Skill {
    pub fn uri(&self) -> String {
        format!("skill://{}/SKILL.md", self.name)
    }

    pub fn digest(&self) -> String {
        format!("sha256:{:x}", Sha256::digest(self.text.as_bytes()))
    }

    /// The skill's entry in `skills/list` and `skills/get`. Each skill is a
    /// single file, so `resources` is just SKILL.md. The top-level `digest` is
    /// not in the final SEP: Claude Code 2.1's client checks it, and a field a
    /// client doesn't know costs it nothing.
    pub fn entry(&self) -> Value {
        json!({
            "uri": self.uri(),
            "frontmatter": { "name": self.name, "description": self.description },
            "resources": [{ "uri": self.uri(), "digest": self.digest(), "size": self.text.len() }],
            "digest": self.digest(),
        })
    }
}

/// Every built-in skill, rendered for one delegation setting.
pub fn render(mode: Delegation) -> Vec<Skill> {
    SOURCES
        .iter()
        .map(|(_, source)| {
            let text = source.replace(DELEGATION_PLACEHOLDER, delegation_sentence(mode));
            let (name, description) = frontmatter(&text)
                .expect("built-in skills keep to plain name/description frontmatter");
            Skill { name, description, text }
        })
        .collect()
}

/// `name` and `description` from the frontmatter. Built-in skills use only
/// those two plain one-line keys, so this is all the YAML they need; the tests
/// hold them to it, because `skills/list` must repeat the frontmatter exactly
/// as a YAML parser would read it.
fn frontmatter(text: &str) -> Option<(String, String)> {
    let (block, _) = text.strip_prefix("---\n")?.split_once("\n---\n")?;
    let (mut name, mut description) = (None, None);
    for line in block.lines() {
        match line.split_once(": ")? {
            ("name", value) => name = Some(value.to_string()),
            ("description", value) => description = Some(value.to_string()),
            _ => return None,
        }
    }
    Some((name?, description?))
}

/// agentskills.io: 1-64 lowercase letters, digits and hyphens, no hyphen at
/// either end and no double hyphen.
fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

/// One of the user's own skills: a folder holding a SKILL.md.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSkill {
    pub name: String,
    pub dir: PathBuf,
}

/// The user's skills in `dirs`, and why any folder with a SKILL.md was left
/// out. A name already taken, by one of taskrunner's skills or by a skill in
/// an earlier folder, is refused rather than shadowed: which of two same-named
/// skills an agent loads would otherwise be up to the harness.
pub fn user_skills(dirs: &[PathBuf]) -> (Vec<UserSkill>, Vec<String>) {
    let (mut found, mut skipped) = (Vec::<UserSkill>::new(), vec![]);
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            skipped.push(format!("{}: can't read this folder", dir.display()));
            continue;
        };
        let mut folders: Vec<PathBuf> = entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|folder| folder.join("SKILL.md").is_file())
            .collect();
        folders.sort();
        for folder in folders {
            match user_skill_name(&folder, &found) {
                Ok(name) => found.push(UserSkill { name, dir: folder }),
                Err(why) => skipped.push(format!("{}: {why}; not linked", folder.display())),
            }
        }
    }
    (found, skipped)
}

fn user_skill_name(folder: &Path, taken: &[UserSkill]) -> Result<String, String> {
    let text = fs::read_to_string(folder.join("SKILL.md"))
        .map_err(|err| format!("can't read its SKILL.md: {err}"))?;
    let name = frontmatter_name(&text)
        .ok_or_else(|| "its SKILL.md has no name in its frontmatter".to_string())?;
    if !valid_name(&name) {
        return Err(format!(
            "'{name}' is not a valid skill name (lowercase letters, digits, single hyphens)"
        ));
    }
    if folder.file_name().and_then(|n| n.to_str()) != Some(name.as_str()) {
        return Err(format!(
            "its SKILL.md names it '{name}', and a skill's name must match its folder"
        ));
    }
    if SOURCES.iter().any(|(own, _)| *own == name) {
        return Err(format!("'{name}' is the name of one of taskrunner's own skills"));
    }
    if let Some(first) = taken.iter().find(|skill| skill.name == name) {
        let dir = first.dir.parent().unwrap_or(&first.dir);
        return Err(format!("{} already has a skill named '{name}'", dir.display()));
    }
    Ok(name)
}

/// The top-level `name` of a user's SKILL.md. A user skill may use any
/// agentskills.io key, `metadata` being a nested map, so unlike `frontmatter`
/// this reads only the one line it needs.
fn frontmatter_name(text: &str) -> Option<String> {
    let (block, _) = text.strip_prefix("---\n")?.split_once("\n---")?;
    let value = block.lines().find_map(|line| line.strip_prefix("name:"))?;
    let name = value.trim().trim_matches(|c| c == '"' || c == '\'');
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes() -> [Delegation; 2] {
        [Delegation::Suggest, Delegation::OnRequest]
    }

    #[test]
    fn every_skill_follows_the_agent_skills_format() {
        for mode in modes() {
            for (skill, (dir, _)) in render(mode).iter().zip(SOURCES) {
                assert_eq!(skill.name, dir, "a skill's name must match its directory");
                assert!(valid_name(&skill.name), "invalid skill name {}", skill.name);
                assert!((1..=1024).contains(&skill.description.chars().count()));
                assert!(!skill.text.contains(DELEGATION_PLACEHOLDER));
            }
        }
    }

    /// Plain YAML scalars only: no `: `, no ` #`, no leading indicator. Anything
    /// else would make the JSON frontmatter differ from what a YAML parser reads.
    #[test]
    fn descriptions_are_plain_yaml_scalars() {
        for skill in modes().into_iter().flat_map(render) {
            let d = &skill.description;
            assert!(!d.contains(": ") && !d.contains(" #"), "{}: {d}", skill.name);
            assert!(!d.starts_with(|c: char| "-?:,[]{}#&*!|>'\"%@`".contains(c)), "{}", skill.name);
        }
    }

    #[test]
    fn the_delegation_setting_changes_only_delegate_task() {
        let (suggest, on_request) = (render(Delegation::Suggest), render(Delegation::OnRequest));
        for (a, b) in suggest.iter().zip(&on_request) {
            assert_eq!(a.text == b.text, a.name != "delegate-task", "{}", a.name);
        }
        let delegate = on_request.iter().find(|s| s.name == "delegate-task").unwrap();
        assert!(delegate.description.contains("only when the user explicitly asks"));
    }

    #[test]
    fn an_entry_describes_the_bytes_it_serves() {
        let skill = &render(Delegation::Suggest)[0];
        let entry = skill.entry();
        let expected = format!("sha256:{:x}", Sha256::digest(skill.text.as_bytes()));
        assert_eq!(entry["uri"], format!("skill://{}/SKILL.md", skill.name));
        assert_eq!(entry["frontmatter"]["name"], skill.name);
        assert_eq!(entry["resources"][0]["digest"], expected);
        assert_eq!(entry["resources"][0]["size"], skill.text.len());
        assert_eq!(entry["digest"], expected);
    }

    #[test]
    fn reads_a_user_skills_name_whatever_else_its_frontmatter_holds() {
        let text =
            "---\nname: librarian\ndescription: Docs.\nmetadata:\n  name: nested\n---\nBody\n";
        assert_eq!(frontmatter_name(text).as_deref(), Some("librarian"));
        assert_eq!(frontmatter_name("---\nname: \"quoted\"\n---").as_deref(), Some("quoted"));
        assert_eq!(frontmatter_name("---\ndescription: no name\n---\n"), None);
        assert_eq!(frontmatter_name("name: outside frontmatter\n"), None);
    }
}
