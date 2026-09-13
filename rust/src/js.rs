//! JavaScript semantics, in one place.
//!
//! The index and every rendered view must match the TypeScript program byte
//! for byte, and a handful of its operations differ from Rust's at the edges:
//! `\s` and `trim` use JavaScript's whitespace class, `length`, `slice` and
//! `padEnd` count UTF-16 code units, `String(n)` prints integral numbers
//! without a fraction, and `a ?? b` falls through on null as well as on a
//! missing key. Everything that depends on those goes through here so the
//! difference is visible instead of scattered.

use serde_json::{Map, Value};

/// JavaScript's `\s` (and `trim`) whitespace class. Rust's `is_whitespace`
/// differs at the edges (NEL is in, the BOM is out), so it is spelled out.
pub fn is_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// What `^` matches after in a multiline regex.
pub fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

pub fn trim_start(text: &str) -> &str {
    text.trim_start_matches(is_whitespace)
}

pub fn trim_end(text: &str) -> &str {
    text.trim_end_matches(is_whitespace)
}

pub fn trim(text: &str) -> &str {
    text.trim_matches(is_whitespace)
}

/// `text.replace(/\s+/g, " ").trim()`.
pub fn collapse_whitespace(text: &str) -> String {
    text.split(is_whitespace).filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" ")
}

/// `text.length`.
pub fn len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `text.slice(0, max)`.
pub fn slice_to(text: &str, max: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().take(max).collect();
    String::from_utf16_lossy(&units)
}

/// `text.padEnd(width)`.
pub fn pad_end(text: &str, width: usize) -> String {
    let missing = width.saturating_sub(len(text));
    format!("{text}{}", " ".repeat(missing))
}

/// `Array.prototype.sort()` on strings: UTF-16 code unit order.
pub fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `String(n)`: integers print without a fraction.
pub fn number(n: &serde_json::Number) -> String {
    match n.as_f64() {
        Some(f) if f.fract() == 0.0 && f.abs() < 1e21 => format!("{}", f as i128),
        _ => n.to_string(),
    }
}

// ---- Loose JSON access ---------------------------------------------------

/// `blob[keys[0]] ?? blob[keys[1]] ?? …`: the first key present with a
/// non-null value.
pub fn first_present<'a>(blob: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| blob.get(*key)).filter(|value| !value.is_null())
}

/// JSON objects arrive both parsed and as nested strings (codex `arguments`).
pub fn parse_object(value: &Value) -> Option<Map<String, Value>> {
    match value {
        Value::String(text) => parse_object_text(text),
        Value::Object(map) => Some(map.clone()),
        _ => None,
    }
}

pub fn parse_object_text(text: &str) -> Option<Map<String, Value>> {
    parse_object(&serde_json::from_str(text).ok()?)
}
