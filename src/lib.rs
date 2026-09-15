//! Taskrunner: hands tasks to coding-agent workers in isolated Docker
//! containers and keeps a durable, searchable audit archive of what they did.

pub mod cli;
pub mod client;
pub mod config;
pub mod daemon;
pub mod doctor;
pub mod domain;
pub mod harnesses;
pub mod ids;
pub mod ingest;
pub mod js;
pub mod paths;
pub mod process;
pub mod shim;
pub mod skills;
pub mod storage;
pub mod sync;
pub mod view;
pub mod workers;
pub mod workspace;
