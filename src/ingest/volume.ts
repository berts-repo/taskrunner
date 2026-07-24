import { spawn } from "node:child_process";
import * as fs from "node:fs";

// Copies a subtree out of a Docker named volume onto the host so the
// transcript sweeper can read it. Worker auth volumes are not mountable from
// the host on macOS (VirtioFS), so we go through Docker: create a container
// with the volume mounted read-only, `docker cp` the subtree out, remove the
// container. `docker cp` reads a *created* (never-started) container's
// filesystem, so nothing needs to run and the image needs no `tar` — any
// already-built worker image works as the mount vehicle.
//
// Every step is async on purpose. A synchronous copy-out would block the
// event loop for the whole of `create` + `cp` + `rm` on every sweep, which is
// the same defect that made the first ingest build unbootable — and here it
// would strike after the daemon is already serving, so no startup ordering
// saves us.

/**
 * Copies `<volume>/<subdir>` into `destDir` (created if missing, owner-only).
 * Rejects on any docker failure so the caller can log and skip the source.
 */
export async function dockerCopyOut(
  volume: string,
  subdir: string,
  destDir: string,
  image: string,
  dockerCommand = "docker",
): Promise<void> {
  // Staging holds transcript content copied out of an auth volume; keep it
  // readable only by the user running the daemon.
  fs.mkdirSync(destDir, { recursive: true, mode: 0o700 });
  fs.chmodSync(destDir, 0o700);
  const created = await run(dockerCommand, ["create", "-v", `${volume}:/v:ro`, image, "true"]);
  const containerId = created.stdout.trim();
  if (!created.ok || !containerId) {
    throw new Error(`docker create failed for volume ${volume}: ${created.stderr.trim()}`);
  }
  try {
    // The trailing "/." copies the directory's contents into destDir rather
    // than nesting it under a <subdir> child.
    const copied = await run(dockerCommand, ["cp", `${containerId}:/v/${subdir}/.`, destDir]);
    if (!copied.ok) {
      // A missing subdir (worker logged in but never ran) is not an error:
      // there is simply nothing to ingest yet.
      if (/no such file or directory|not found/i.test(copied.stderr)) return;
      throw new Error(`docker cp failed for ${volume}/${subdir}: ${copied.stderr.trim()}`);
    }
  } finally {
    await run(dockerCommand, ["rm", "-f", containerId]);
  }
}

function run(
  command: string,
  args: string[],
): Promise<{ ok: boolean; stdout: string; stderr: string }> {
  return new Promise((resolve) => {
    const child = spawn(command, args, { timeout: 60_000 });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => (stdout += chunk));
    child.stderr.on("data", (chunk: string) => (stderr += chunk));
    // `error` fires instead of `close` when the binary is missing entirely.
    child.on("error", (err) => resolve({ ok: false, stdout, stderr: stderr || err.message }));
    child.on("close", (code) => resolve({ ok: code === 0, stdout, stderr }));
  });
}
