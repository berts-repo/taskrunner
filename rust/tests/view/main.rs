//! View tests: one module per TypeScript test file. Each module seeds its own
//! index the way the original did, timestamps and all, because the renderers
//! print those timestamps and the assertions are on the exact text.

#[path = "../helpers/mod.rs"]
mod helpers;

mod lookup;
mod outline;
mod seed;
mod timeline;
mod transcript;
