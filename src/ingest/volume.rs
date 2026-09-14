//! Copies a subtree out of a Docker named volume onto the host so the
//! transcript sweeper can read it. Worker auth volumes are not mountable from
//! the host on macOS (VirtioFS), so we go through Docker: create a container
//! with the volume mounted read-only, `docker cp` the subtree out, remove the
//! container. `docker cp` reads a *created* (never-started) container's
//! filesystem, so nothing needs to run and the image needs no `tar` — any
//! already-built worker image works as the mount vehicle.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use anyhow::bail;

use crate::process::{Output, run as run_with_timeout};

/// Marks the throwaway mount containers so a later daemon can recognize and
/// reap the ones an earlier one left behind. Filtering on this label is what
/// keeps the reap from ever touching a worker container.
pub const COPY_OUT_LABEL: &str = "taskrunner.role=ingest-copyout";

const DOCKER_TIMEOUT: Duration = Duration::from_secs(60);

/// Copies `<volume>/<subdir>` into `dest_dir` (created if missing,
/// owner-only). Fails on any docker failure so the caller can log and skip
/// the source.
pub fn docker_copy_out(
    volume: &str,
    subdir: &str,
    dest_dir: &Path,
    image: &str,
    docker: &str,
) -> anyhow::Result<()> {
    // Staging holds transcript content copied out of an auth volume; keep it
    // readable only by the user running the daemon.
    fs::create_dir_all(dest_dir)?;
    fs::set_permissions(dest_dir, fs::Permissions::from_mode(0o700))?;
    let created = run(
        docker,
        &["create", "--label", COPY_OUT_LABEL, "-v", &format!("{volume}:/v:ro"), image, "true"],
    );
    let container_id = created.text().trim().to_string();
    if !created.ok || container_id.is_empty() {
        bail!("docker create failed for volume {volume}: {}", created.stderr.trim());
    }
    // The trailing "/." copies the directory's contents into dest_dir rather
    // than nesting it under a <subdir> child.
    let copied =
        run(docker, &["cp", &format!("{container_id}:/v/{subdir}/."), &dest_dir.to_string_lossy()]);
    run(docker, &["rm", "-f", &container_id]);
    if !copied.ok {
        // A missing subdir (worker logged in but never ran) is not an error:
        // there is simply nothing to ingest yet.
        let stderr = copied.stderr.to_lowercase();
        if stderr.contains("no such file or directory") || stderr.contains("not found") {
            return Ok(());
        }
        bail!("docker cp failed for {volume}/{subdir}: {}", copied.stderr.trim());
    }
    Ok(())
}

/// Removes copy-out containers stranded by an earlier daemon and returns how
/// many went away. `docker_copy_out` deletes its own container on every path
/// it controls, but a SIGKILL between `create` and that cleanup leaves one
/// behind forever — `--rm` is no help, because auto-remove fires when a
/// container exits and these never start. So the next daemon reaps them.
///
/// Startup only, never mid-sweep: a running copy-out's container matches the
/// same label, and pulling it out from under an in-flight `docker cp` would
/// fail the sweep it belongs to.
pub fn reap_copy_out_containers(docker: &str) -> anyhow::Result<usize> {
    let listed = run(docker, &["ps", "-aq", "--filter", &format!("label={COPY_OUT_LABEL}")]);
    if !listed.ok {
        bail!("docker ps failed while reaping copy-out containers: {}", listed.stderr.trim());
    }
    let listed = listed.text();
    let ids: Vec<&str> = listed.lines().map(str::trim).filter(|id| !id.is_empty()).collect();
    if ids.is_empty() {
        return Ok(0);
    }
    let removed = run(docker, &[&["rm", "-f"][..], &ids[..]].concat());
    if !removed.ok {
        bail!("docker rm failed while reaping copy-out containers: {}", removed.stderr.trim());
    }
    Ok(ids.len())
}

/// Every docker call here is bounded: a wedged daemon must not wedge a sweep.
fn run(command: &str, args: &[&str]) -> Output {
    run_with_timeout(command, args, DOCKER_TIMEOUT)
}
