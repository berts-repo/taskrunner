//! Tamper evidence for the event log.
//!
//! Every line's fingerprint is the SHA-256 of the previous line's fingerprint
//! followed by the line's own bytes, so a fingerprint depends on every line
//! before it and on nothing after. A line records the fingerprint before it in
//! `prev`. Lines written before chaining started have no `prev`, and are still
//! covered: the first line that has one depends on all of them.
//!
//! A chain alone cannot catch someone who edits a line and recomputes every
//! link after it. An anchor can: a fingerprint kept somewhere else, which the
//! rewritten log no longer reproduces. The daemon keeps anchors in
//! `anchors.jsonl` beside the log; `taskrunner anchor` prints one for the user
//! to keep off the machine.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How many durable events may pass between automatic anchors.
pub const ANCHOR_EVERY: u64 = 100;

/// The fingerprint after `line`, given the one before it. The first line has
/// nothing before it, so `prev` is empty there.
pub fn fingerprint(prev: &str, line: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prev.as_bytes());
    hasher.update(line);
    format!("sha256:{:x}", hasher.finalize())
}

pub fn anchors_path(events_log: &Path) -> PathBuf {
    events_log.with_file_name("anchors.jsonl")
}

/// The log's fingerprint after one event. `event` counts lines from 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub event: u64,
    pub id: String,
    pub ts: String,
    pub fingerprint: String,
}

impl Anchor {
    /// The line `taskrunner anchor` prints and `Claim::parse` reads back.
    pub fn display(&self) -> String {
        format!("event {}  {}  {}", self.event, self.ts, self.fingerprint)
    }
}

/// The first line where the chain stops holding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Break {
    /// Its `prev` is not the fingerprint of the lines before it.
    WrongLink { event: u64 },
    /// It has no `prev`, though an earlier line started the chain.
    Unlinked { event: u64 },
}

/// What one pass over the log's complete lines found.
#[derive(Debug, Default)]
pub struct Walk {
    pub events: u64,
    /// The fingerprint after the last line; empty for an empty log.
    pub head: String,
    pub last_id: String,
    pub last_ts: String,
    pub chain_starts: Option<u64>,
    pub first_break: Option<Break>,
}

impl Walk {
    pub fn anchor(&self) -> Option<Anchor> {
        (self.events > 0).then(|| Anchor {
            event: self.events,
            id: self.last_id.clone(),
            ts: self.last_ts.clone(),
            fingerprint: self.head.clone(),
        })
    }
}

/// The fields the chain reads from a line; everything else is only hashed.
#[derive(Default, Deserialize)]
struct LineFields {
    id: Option<String>,
    ts: Option<String>,
    prev: Option<String>,
}

/// Reads every complete line, calling `visit` with each event number and the
/// fingerprint after it. An unterminated last line is a torn write the daemon
/// discards on its next start, not an event.
pub fn walk(events_log: &Path, mut visit: impl FnMut(u64, &str)) -> io::Result<Walk> {
    let file = match File::open(events_log) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Walk::default()),
        Err(err) => return Err(err),
    };
    let mut reader = BufReader::new(file);
    let mut walk = Walk::default();
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 || line.last() != Some(&b'\n') {
            return Ok(walk);
        }
        line.pop();
        let event = walk.events + 1;
        // A line that is not JSON still counts and is still hashed; it just
        // cannot carry a link.
        let fields: LineFields = serde_json::from_slice(&line).unwrap_or_default();
        match (&fields.prev, walk.chain_starts) {
            (Some(prev), _) => {
                walk.chain_starts.get_or_insert(event);
                if *prev != walk.head {
                    walk.first_break.get_or_insert(Break::WrongLink { event });
                }
            }
            (None, Some(_)) => {
                walk.first_break.get_or_insert(Break::Unlinked { event });
            }
            (None, None) => {}
        }
        walk.head = fingerprint(&walk.head, &line);
        walk.events = event;
        walk.last_id = fields.id.unwrap_or_default();
        walk.last_ts = fields.ts.unwrap_or_default();
        visit(event, &walk.head);
    }
}

/// The anchor for the log as it stands now, or None when it is empty.
pub fn current(events_log: &Path) -> io::Result<Option<Anchor>> {
    Ok(walk(events_log, |_, _| {})?.anchor())
}

/// Appends one anchor and forces it to disk. The file is only ever opened for
/// appending, so this keeps working once it is marked append-only.
pub fn write_anchor(anchors: &Path, anchor: &Anchor) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).read(true).append(true).open(anchors)?;
    let mut text = String::new();
    // A crash mid-write can leave a torn last line; start a new line rather
    // than extend it into something unreadable.
    let len = file.metadata()?.len();
    if len > 0 {
        let mut last = [0u8; 1];
        file.seek(SeekFrom::Start(len - 1))?;
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            text.push('\n');
        }
    }
    text.push_str(&serde_json::to_string(anchor).expect("anchors always serialize"));
    text.push('\n');
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

#[derive(Debug, Default)]
pub struct AnchorsFile {
    pub anchors: Vec<Anchor>,
    /// Line numbers (from 1) that are not anchors.
    pub unreadable: Vec<usize>,
}

pub fn read_anchors(anchors: &Path) -> io::Result<AnchorsFile> {
    let text = match fs::read_to_string(anchors) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(AnchorsFile::default()),
        Err(err) => return Err(err),
    };
    let mut file = AnchorsFile::default();
    for (index, line) in text.lines().enumerate().filter(|(_, line)| !line.is_empty()) {
        match serde_json::from_str(line) {
            Ok(anchor) => file.anchors.push(anchor),
            Err(_) => file.unreadable.push(index + 1),
        }
    }
    Ok(file)
}

/// An anchor the user kept: the line `taskrunner anchor` printed, or only its
/// fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub event: Option<u64>,
    pub fingerprint: String,
}

impl Claim {
    pub fn parse(text: &str) -> Option<Claim> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let fingerprint = words.iter().find(|word| is_fingerprint(word))?.to_ascii_lowercase();
        let event =
            words.windows(2).find(|pair| pair[0] == "event").and_then(|pair| pair[1].parse().ok());
        Some(Claim { event, fingerprint })
    }
}

fn is_fingerprint(word: &str) -> bool {
    word.strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

#[derive(Debug)]
pub struct Report {
    pub walk: Walk,
    pub anchors_checked: usize,
    /// The event a saved anchor matched, when one was given and matched.
    pub claim_matched: Option<u64>,
    /// Everything that failed, in plain words. Empty means verified.
    pub problems: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    pub fn summary(&self) -> String {
        let chain = match self.walk.chain_starts {
            Some(start) => format!("chained from event {start}"),
            None => "not chained yet".to_string(),
        };
        let anchors = match self.anchors_checked {
            1 => "1 anchor checked".to_string(),
            n => format!("{n} anchors checked"),
        };
        format!("{} events, {chain}, {anchors}", self.walk.events)
    }

    pub fn render(&self) -> String {
        let mut out = format!("Event log: {}\n", self.summary());
        if let Some(event) = self.claim_matched {
            out.push_str(&format!("Your anchor: matches event {event}\n"));
        }
        let evidence = self.walk.chain_starts.is_some()
            || self.anchors_checked > 0
            || self.claim_matched.is_some();
        if self.ok() && !evidence {
            out.push_str(
                "Nothing to check against yet: links and anchors start the next time the daemon \
                 opens the log.\n",
            );
        } else if self.ok() {
            out.push_str("Verified: the chain is intact and every anchor matches.\n");
        } else {
            out.push_str("NOT verified:\n");
            for problem in &self.problems {
                out.push_str(&format!("  - {problem}\n"));
            }
        }
        out
    }
}

/// Checks the chain, every anchor in the anchors file, and `claim` if given,
/// in one pass over the log.
pub fn verify(events_log: &Path, claim: Option<&Claim>) -> io::Result<Report> {
    let anchors = read_anchors(&anchors_path(events_log))?;
    let wanted: BTreeSet<u64> =
        anchors.anchors.iter().map(|a| a.event).chain(claim.and_then(|c| c.event)).collect();
    let mut fingerprints_at: BTreeMap<u64, String> = BTreeMap::new();
    let mut claim_seen_at = None;
    let walk = walk(events_log, |event, fingerprint| {
        if wanted.contains(&event) {
            fingerprints_at.insert(event, fingerprint.to_string());
        }
        if claim.is_some_and(|c| c.fingerprint == fingerprint) {
            claim_seen_at.get_or_insert(event);
        }
    })?;

    let mut problems = Vec::new();
    match walk.first_break {
        Some(Break::WrongLink { event }) => problems.push(format!(
            "the chain breaks at event {event}: its link does not match the events before it, \
             so something at or before event {} changed",
            event - 1
        )),
        Some(Break::Unlinked { event }) => problems.push(format!(
            "event {event} has no link although the chain started earlier: it was inserted or \
             rewritten"
        )),
        None => {}
    }
    for anchor in &anchors.anchors {
        match fingerprints_at.get(&anchor.event) {
            None => problems.push(format!(
                "an anchor records event {}, but the log now ends at event {}: events were removed",
                anchor.event, walk.events
            )),
            Some(now) if *now != anchor.fingerprint => problems.push(format!(
                "the anchor for event {} ({}) no longer matches: history up to that event changed",
                anchor.event, anchor.ts
            )),
            Some(_) => {}
        }
    }
    for line in &anchors.unreadable {
        problems.push(format!(
            "line {line} of the anchors file is not an anchor: it was edited, or a crash tore it"
        ));
    }

    let mut claim_matched = None;
    if let Some(claim) = claim {
        match claim.event {
            Some(event) => match fingerprints_at.get(&event) {
                None => problems.push(format!(
                    "your anchor is for event {event}, but the log ends at event {}: events were \
                     removed",
                    walk.events
                )),
                Some(now) if *now != claim.fingerprint => problems.push(format!(
                    "your anchor for event {event} no longer matches: history up to that event \
                     changed"
                )),
                Some(_) => claim_matched = Some(event),
            },
            None => match claim_seen_at {
                Some(event) => claim_matched = Some(event),
                None => problems.push(
                    "no point in the log matches your anchor: history up to it changed, or it \
                     came from a different log"
                        .to_string(),
                ),
            },
        }
    }

    Ok(Report { walk, anchors_checked: anchors.anchors.len(), claim_matched, problems })
}
