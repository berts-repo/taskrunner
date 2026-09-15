// Shapes here are copied from real archived records: claude-code writes
// {id,name,input} / {tool_use_id,is_error,content}; codex writes
// {call_id,name,arguments} / {call_id,output} with arguments as a JSON string,
// or {call_id,name,input} for exec, whose input is a JavaScript program.

use serde_json::{Value, json};
use taskrunner::storage::facts::{MessageFacts, message_facts};

fn claude_use(name: &str, input: Value) -> String {
    json!({ "id": "toolu_01VQbWiHWjEA3CStjPxUrfSt", "name": name, "input": input }).to_string()
}

fn use_facts(name: &str, input: Value) -> MessageFacts {
    message_facts("assistant", "tool_use", &claude_use(name, input))
}

fn result_facts(content: Value) -> MessageFacts {
    message_facts("tool", "tool_result", &content.to_string())
}

#[test]
fn reads_id_name_and_target_off_a_claude_code_tool_use() {
    let facts = use_facts("Read", json!({ "file_path": "/a/b.ts" }));
    assert_eq!(facts.tool_use_id.as_deref(), Some("toolu_01VQbWiHWjEA3CStjPxUrfSt"));
    assert_eq!(facts.tool_name.as_deref(), Some("Read"));
    assert_eq!(facts.tool_target.as_deref(), Some("/a/b.ts"));
    assert_eq!(facts.is_error, None);
}

#[test]
fn reads_call_id_name_and_target_off_a_codex_tool_use() {
    let content = json!({
        "call_id": "call_rMXEMl9x6Wc7H9ujPIY6syLY",
        "name": "exec_command",
        "arguments": json!({ "cmd": "ls -la ~/.codex" }).to_string(),
    });
    let facts = message_facts("assistant", "tool_use", &content.to_string());
    assert_eq!(facts.tool_use_id.as_deref(), Some("call_rMXEMl9x6Wc7H9ujPIY6syLY"));
    assert_eq!(facts.tool_name.as_deref(), Some("exec_command"));
    assert_eq!(facts.tool_target.as_deref(), Some("ls -la ~/.codex"));
}

#[test]
fn pairs_a_codex_exec_call_with_its_output_but_takes_no_target_from_its_program() {
    // exec's input is a JavaScript program that may run several tools, so no
    // one identifier names what it acted on. The program stays searchable.
    let call = json!({
        "call_id": "call_7x6PhysrjXuQGZ6ovHfcRKi8",
        "name": "exec",
        "input": "text(await tools.exec_command({cmd:\"rg -n XDG src\"}));\n",
    });
    let facts = message_facts("assistant", "tool_use", &call.to_string());
    assert_eq!(facts.tool_use_id.as_deref(), Some("call_7x6PhysrjXuQGZ6ovHfcRKi8"));
    assert_eq!(facts.tool_name.as_deref(), Some("exec"));
    assert_eq!(facts.tool_target, None);

    let result = result_facts(json!({
        "call_id": "call_7x6PhysrjXuQGZ6ovHfcRKi8",
        "output": "Script completed\nWall time 0.2 seconds\nOutput:\n\n{\"exit_code\":2}",
    }));
    assert_eq!(result.tool_use_id.as_deref(), Some("call_7x6PhysrjXuQGZ6ovHfcRKi8"));
    assert_eq!(result.is_error, None);
}

#[test]
fn prefers_a_path_over_a_command_and_flattens_multiline_and_argv_targets() {
    let target = |name, input| use_facts(name, input).tool_target;
    assert_eq!(
        target("Edit", json!({ "file_path": "/x.ts", "command": "rm" })).as_deref(),
        Some("/x.ts")
    );
    assert_eq!(
        target("Bash", json!({ "command": "grep -n foo \\\n  bar" })).as_deref(),
        Some("grep -n foo \\ bar")
    );
    assert_eq!(
        target("shell", json!({ "command": ["bash", "-lc", "ls"] })).as_deref(),
        Some("bash -lc ls")
    );
}

#[test]
fn leaves_the_target_none_when_a_tool_takes_only_prose() {
    let facts = use_facts("AskUserQuestion", json!({ "questions": [] }));
    assert_eq!(facts.tool_name.as_deref(), Some("AskUserQuestion"));
    assert_eq!(facts.tool_target, None);
}

#[test]
fn pairs_a_tool_result_back_to_its_call_and_records_the_failure_flag() {
    let ok =
        result_facts(json!({ "tool_use_id": "toolu_1", "is_error": false, "content": "done" }));
    assert_eq!(ok.tool_use_id.as_deref(), Some("toolu_1"));
    assert_eq!(ok.is_error, Some(0));
    assert_eq!(ok.tool_name, None);
    let bad =
        result_facts(json!({ "tool_use_id": "toolu_1", "is_error": true, "content": "boom" }));
    assert_eq!(bad.is_error, Some(1));
}

#[test]
fn derives_failure_from_both_codex_exec_preambles_and_stays_none_otherwise() {
    let result =
        |output: &str| result_facts(json!({ "call_id": "call_1", "output": output })).is_error;
    assert_eq!(result("Exit code: 0\nWall time: 0.1 seconds\nOutput:\nhi\n"), Some(0));
    assert_eq!(result("Exit code: 127\nWall time: 0.1 seconds\nOutput:\nnot found\n"), Some(1));
    assert_eq!(
        result("Chunk ID: 3026\nWall time: 0.05 seconds\nProcess exited with code 0\n"),
        Some(0)
    );
    assert_eq!(
        result("Chunk ID: 3026\nWall time: 0.05 seconds\nProcess exited with code 2\n"),
        Some(1)
    );
    // Rejections and aborts say nothing about success: None, not a guess.
    assert_eq!(result("exec command rejected by user"), None);
    assert_eq!(result("Wall time: 9.6 seconds\naborted by user"), None);
}

#[test]
fn survives_unparseable_or_unfamiliar_content_without_failing() {
    let facts = message_facts("assistant", "tool_use", "not json");
    assert_eq!(facts.tool_use_id, None);
    assert_eq!(facts.tool_name, None);
    assert_eq!(facts.tool_target, None);
    assert_eq!(message_facts("assistant", "tool_use", r#"{"foo":1}"#).tool_name, None);
}

#[test]
fn counts_a_typed_user_turn_as_a_prompt() {
    assert!(message_facts("user", "message", "interview for next phase").is_prompt);
    assert!(message_facts("user", "message", "  continue\n").is_prompt);
}

#[test]
fn rejects_the_harness_written_user_records_that_would_corrupt_the_numbering() {
    let pseudo = [
        "<command-name>/clear</command-name>\n<command-message>clear</command-message>",
        "<local-command-caveat>Caveat: The messages below were generated by the user…",
        "<local-command-stdout></local-command-stdout>",
        "<task-notification>agent finished</task-notification>",
        "<bash-input>ls</bash-input>",
        "<system-reminder>remember to…</system-reminder>",
        "[Request interrupted by user for tool use]",
        "<environment_context>\n  <cwd>/Users/iBert</cwd>\n</environment_context>",
        "<turn_aborted>\nThe user interrupted the previous turn on purpose.",
        "<recommended_plugins>\nHere is a list of plugins…",
        "# AGENTS.md instructions for /Users/iBert/Documents\n<INSTRUCTIONS>",
        "",
    ];
    for content in pseudo {
        assert!(!message_facts("user", "message", content).is_prompt, "{content:.30}");
    }
}

#[test]
fn counts_nothing_but_a_user_message_as_a_prompt() {
    assert!(!message_facts("assistant", "message", "sure thing").is_prompt);
    assert!(!message_facts("user", "tool_result", "{}").is_prompt);
    assert!(!message_facts("system", "system", "hook fired").is_prompt);
}
