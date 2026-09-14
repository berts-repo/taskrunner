//! Transcript ingestion: parsers turn each harness's own files into
//! normalized messages, and the sweeper appends the ones the archive has not
//! seen.

pub mod claude_code;
pub mod codex;
pub mod parser;
pub mod registry;
pub mod sweep;
pub mod volume;
