# NAS storage — proposal (not implemented)

Noted 2026-09-15. Nothing here is built yet; this captures the direction before it gets
lost.

## The goal

Keep Taskrunner's record on the owner's NAS, reached over Tailscale, so it survives this
laptop dying and can be read from another machine. Build it so the NAS can later be
swapped for a cloud store without redesigning.

## What lives in `~/.taskrunner` today

`src/paths.rs` puts everything under one root (146 MB on 2026-09-15):

| Path | Size | What it is | Needs to leave the machine? |
|---|---|---|---|
| `events.jsonl` | 21 MB | Append-only, hash-chained log. **The source of truth.** | ✅ Yes |
| `anchors.jsonl` | 4 KB | Fingerprints of the log | ✅ Yes |
| `artifacts/`, `corpus/` | 5 MB | Content-addressed diffs and worker streams | ✅ Yes |
| `config.toml` | 4 KB | Owner settings | ✅ Yes |
| `index.db` (+ `-wal`, `-shm`) | 96 MB | SQLite index, rebuilt from the log on every boot | ❌ No, derived |
| `runtime/` | — | Unix sockets, pid and lock files | ❌ No, must stay local |
| `workspaces/` | 24 MB | Task clones | Open question, see below |
| `ingest-staging/`, `ingest-state.json`, `logs/`, `skills/` | — | Caches and rendered output | ❌ No |

## Considered and rejected: mount the NAS and point the root at it

The obvious move is to mount a NAS share and put all of `~/.taskrunner` on it. Rejected:

- **SQLite over a network filesystem corrupts.** WAL mode needs shared memory and
  file locks that NFS and SMB don't honour reliably. SQLite's own docs warn against it.
- **Unix sockets don't work on network mounts**, and `src/daemon/lock.rs` takes its lock
  with a hard link, which SMB shares often don't support.
- **The daemon would stall whenever the tailnet drops** — on a train, with the NAS
  asleep, or with Tailscale off. Every task and every MCP call would hang on I/O.

## Recommended: local primary, NAS replica of the log

The daemon keeps writing locally, exactly as today. The durable files (the ✅ rows above)
are copied to the NAS over Tailscale after they're written.

- **The log's design makes this cheap.** It is append-only, so a sync only ships the new
  tail. The index is derived, so it is never copied: a machine reading the replica
  rebuilds its own.
- **The replica never rewrites bytes it already has.** It only appends. If someone later
  edits old entries on the laptop, the NAS keeps the original, and `taskrunner verify`
  run against the replica fails from the edited entry on. This makes the NAS the
  off-machine anchor that [log integrity](../../guide/log-integrity.md) says closes the
  root-on-this-machine gap.
- **It works offline.** If the NAS is unreachable, the sync catches up next time; the
  daemon never waits on it.

### Steps, smallest first

1. **Manual, no code.** A systemd user timer runs `rsync --append` (not
   `--append-verify`, which rewrites a changed file) of the durable files to the NAS's
   Tailscale name. Document it in a guide page.
2. **Built in, if step 1 proves useful.** A `[replica]` table in `config.toml` with a
   target, plus `taskrunner replica sync` and `taskrunner replica status` (last synced
   event, lag). The daemon may trigger a sync after anchoring, never block on one.

### Porting to a cloud store later

Object stores (S3, Backblaze B2) can't append to a file. So the built-in version should
ship **segments**: "the log bytes from offset X to Y" as one new immutable file or object.
On a NAS a segment can be appended to one file; in a bucket each segment is its own
object. The same sync code then serves both, and a bucket with object lock gives the
same "can't rewrite the past" property as append-only.

## Security notes

- **The replica holds full transcripts**, which can include pasted secrets. Restrict the
  NAS share with a Tailscale ACL to this machine, and encrypt before upload for any
  cloud target (the provider should see only ciphertext).
- **A push from the laptop means the laptop can also delete the replica.** NAS snapshots
  (read-only point-in-time copies) are what stop a compromised laptop from wiping it.
  Update `docs/security/overview.md` when this ships.

## Open questions

- Which NAS and share type (NFS, SMB, or rsync over SSH)? Does it support snapshots?
- Are unmerged `taskrunner/<task-id>` branches only in `workspaces/`? If so, losing the
  laptop loses unreviewed work, and workspaces need a place in the replica too.
- Reading the replica from a second machine: read-only daemon, or just `verify` and
  search?
