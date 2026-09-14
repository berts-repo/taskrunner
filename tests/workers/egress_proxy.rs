//! The egress proxy is still the Node script its sidecar image runs, so its
//! tests are Node's own (`docker/egress-proxy/server.test.cjs`). Running them
//! from here keeps `cargo test` the one command that runs every test.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn egress_proxy_node_tests_pass() {
    let tests =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docker/egress-proxy/server.test.cjs");
    let output = Command::new("node").arg("--test").arg(&tests).output().expect("node is on PATH");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
