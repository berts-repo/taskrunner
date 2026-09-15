//! Tamper evidence: any change to the log's history must fail `verify`, and
//! an untouched log must pass however much is appended to it.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use taskrunner::storage::chain::{self, ANCHOR_EVERY, Claim, anchors_path, read_anchors, verify};
use taskrunner::storage::events::{EventBody, EventLog, read_events};

use crate::helpers::evt;

fn session_ended(n: u64) -> EventBody {
    EventBody::SessionEnded { session_id: format!("sess_{n}") }
}

/// A log of `n` events, each appended and fsynced as the daemon writes them.
fn log_with(dir: &Path, n: u64) -> PathBuf {
    let path = dir.join("events.jsonl");
    let mut log = EventLog::open(&path).unwrap();
    for i in 0..n {
        log.append(session_ended(i)).unwrap();
    }
    path
}

/// A log written before chaining existed: plain events, no links.
fn unchained_log(dir: &Path, n: u64) -> PathBuf {
    let path = dir.join("events.jsonl");
    let lines: Vec<String> =
        (0..n).map(|i| serde_json::to_string(&evt(session_ended(i))).unwrap()).collect();
    write_lines(&path, &lines);
    path
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path).unwrap().lines().map(str::to_string).collect()
}

fn write_lines(path: &Path, lines: &[String]) {
    fs::write(path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
}

/// Changes one event (numbered from 1) in place, as a hand edit would.
fn edit(path: &Path, event: usize) {
    let mut all = lines(path);
    all[event - 1] = all[event - 1].replace("sess_", "sess_EDITED_");
    write_lines(path, &all);
}

/// Recomputes every link after an edit, as someone covering their tracks would.
fn relink(path: &Path) {
    let mut head = String::new();
    let relinked: Vec<String> = lines(path)
        .into_iter()
        .map(|line| {
            let mut value: Value = serde_json::from_str(&line).unwrap();
            let object = value.as_object_mut().unwrap();
            if object.contains_key("prev") {
                object.insert("prev".into(), head.clone().into());
            }
            let line = value.to_string();
            head = chain::fingerprint(&head, line.as_bytes());
            line
        })
        .collect();
    write_lines(path, &relinked);
}

fn prev_of(line: &str) -> Option<String> {
    serde_json::from_str::<Value>(line).unwrap()["prev"].as_str().map(str::to_string)
}

fn anchored_events(path: &Path) -> Vec<u64> {
    read_anchors(&anchors_path(path)).unwrap().anchors.iter().map(|a| a.event).collect()
}

fn problems(path: &Path, claim: Option<&Claim>) -> Vec<String> {
    verify(path, claim).unwrap().problems
}

#[test]
fn links_each_new_event_to_everything_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = log_with(dir.path(), 3);
    let all = lines(&path);

    assert_eq!(prev_of(&all[0]), None, "nothing precedes the first event");
    let after_first = chain::fingerprint("", all[0].as_bytes());
    assert_eq!(prev_of(&all[1]), Some(after_first.clone()));
    let after_second = chain::fingerprint(&after_first, all[1].as_bytes());
    assert_eq!(prev_of(&all[2]), Some(after_second));

    // The link is invisible to everything that reads events.
    assert_eq!(read_events(&path).unwrap().len(), 3);
    let report = verify(&path, None).unwrap();
    assert!(report.ok(), "{:?}", report.problems);
    assert_eq!(report.walk.chain_starts, Some(2));
}

#[test]
fn covers_history_written_before_chaining_with_the_first_link_and_an_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let path = unchained_log(dir.path(), 2);
    let before = chain::current(&path).unwrap().unwrap();

    let mut log = EventLog::open(&path).unwrap();
    let anchors = read_anchors(&anchors_path(&path)).unwrap().anchors;
    assert_eq!(anchors, vec![before.clone()], "opening anchors the unchained history");

    log.append(session_ended(2)).unwrap();
    assert_eq!(prev_of(&lines(&path)[2]), Some(before.fingerprint));
    assert!(problems(&path, None).is_empty());
}

#[test]
fn does_not_claim_to_have_verified_a_log_with_no_links_or_anchors_yet() {
    let dir = tempfile::tempdir().unwrap();
    let path = unchained_log(dir.path(), 2);
    let report = verify(&path, None).unwrap();
    assert!(report.ok(), "nothing is wrong, so verify does not fail");
    let out = report.render();
    assert!(!out.contains("Verified"), "{out}");
    assert!(out.contains("Nothing to check against yet"), "{out}");
}

#[test]
fn an_edited_event_breaks_the_chain_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = log_with(dir.path(), 3);
    edit(&path, 2);
    let found = problems(&path, None);
    assert!(found.iter().any(|p| p.contains("breaks at event 3")), "{found:?}");
}

#[test]
fn recomputing_the_links_after_an_edit_is_caught_by_an_automatic_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let path = log_with(dir.path(), ANCHOR_EVERY + 20);
    assert_eq!(anchored_events(&path), vec![ANCHOR_EVERY]);

    edit(&path, 50);
    relink(&path);
    let report = verify(&path, None).unwrap();
    assert_eq!(report.walk.first_break, None, "the relinked chain holds on its own");
    assert!(report.problems.iter().any(|p| p.contains("event 100")), "{:?}", report.problems);
}

#[test]
fn an_edit_to_history_from_before_chaining_is_caught_by_the_first_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let path = unchained_log(dir.path(), 2);
    drop(EventLog::open(&path).unwrap());
    edit(&path, 1);
    assert!(!problems(&path, None).is_empty());
}

#[test]
fn removing_events_from_the_end_is_caught() {
    let dir = tempfile::tempdir().unwrap();
    let path = log_with(dir.path(), ANCHOR_EVERY + 20);
    write_lines(&path, &lines(&path)[..90]);
    let found = problems(&path, None);
    assert!(found.iter().any(|p| p.contains("removed")), "{found:?}");
}

#[test]
fn a_saved_anchor_still_matches_after_more_work_and_catches_a_later_edit() {
    let dir = tempfile::tempdir().unwrap();
    let path = log_with(dir.path(), 5);
    let saved = Claim::parse(&chain::current(&path).unwrap().unwrap().display()).unwrap();

    let mut log = EventLog::open(&path).unwrap();
    for i in 5..10 {
        log.append(session_ended(i)).unwrap();
    }
    drop(log);
    let report = verify(&path, Some(&saved)).unwrap();
    assert!(report.ok(), "{:?}", report.problems);
    assert_eq!(report.claim_matched, Some(5));

    // Only the saved anchor stands between this edit and a clean result.
    edit(&path, 3);
    relink(&path);
    fs::remove_file(anchors_path(&path)).unwrap();
    let found = problems(&path, Some(&saved));
    assert!(found.iter().any(|p| p.contains("your anchor")), "{found:?}");
}

#[test]
fn reads_a_saved_anchor_from_the_printed_line_or_the_bare_fingerprint() {
    let hex = "ab".repeat(32);
    let line = format!("event 8612  2026-09-15T06:40:12.818Z  sha256:{hex}");
    assert_eq!(
        Claim::parse(&line),
        Some(Claim { event: Some(8612), fingerprint: format!("sha256:{hex}") })
    );
    assert_eq!(Claim::parse(&format!("sha256:{hex}")).unwrap().event, None);
    assert_eq!(Claim::parse("sha256:abc"), None);
    assert_eq!(Claim::parse("hello"), None);
}

#[test]
fn a_bare_fingerprint_is_found_wherever_it_is_in_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = log_with(dir.path(), 4);
    let saved = chain::current(&path).unwrap().unwrap();
    drop(EventLog::open(&path).unwrap().append(session_ended(4)).unwrap());
    let claim = Claim { event: None, fingerprint: saved.fingerprint };
    assert_eq!(verify(&path, Some(&claim)).unwrap().claim_matched, Some(4));
}

#[test]
fn anchors_every_hundred_durable_events_and_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let mut log = EventLog::open(&path).unwrap();
    for i in 0..250 {
        log.append(session_ended(i)).unwrap();
    }
    assert_eq!(anchored_events(&path), vec![100, 200]);

    log.anchor();
    log.anchor();
    assert_eq!(anchored_events(&path), vec![100, 200, 250], "an anchor is never repeated");
}

#[test]
fn anchors_bulk_writes_only_once_they_are_flushed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let mut log = EventLog::open(&path).unwrap();
    for i in 0..150 {
        log.append_unsynced(session_ended(i)).unwrap();
    }
    assert_eq!(anchored_events(&path), Vec::<u64>::new());
    log.flush().unwrap();
    assert_eq!(anchored_events(&path), vec![150]);
}

#[test]
fn starts_a_new_line_after_a_torn_anchor_instead_of_extending_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = log_with(dir.path(), 1);
    fs::write(anchors_path(&path), r#"{"event":1,"id":"#).unwrap();

    let mut log = EventLog::open(&path).unwrap();
    log.append(session_ended(1)).unwrap();
    log.anchor();
    let file = read_anchors(&anchors_path(&path)).unwrap();
    assert_eq!(file.unreadable, vec![1]);
    assert_eq!(file.anchors.iter().map(|a| a.event).collect::<Vec<_>>(), vec![1, 2]);
}

#[test]
fn an_unterminated_last_line_is_not_an_event() {
    let dir = tempfile::tempdir().unwrap();
    let path = log_with(dir.path(), 2);
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(r#"{"id":"evt_torn","ts":"2026-01-01T00:0"#);
    fs::write(&path, text).unwrap();
    assert_eq!(chain::current(&path).unwrap().unwrap().event, 2);
}
