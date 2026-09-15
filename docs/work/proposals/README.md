# Proposals

Directions discussed with the owner that are not ready to build. When a piece ships,
it leaves its proposal: how it works goes to `docs/guide/` or `docs/reference/`, and
its history goes to [the archive](../archive/).

- [Complete audit](complete-audit.md) — one archive of every conversation across
  harnesses: optional network capture with config/command controls, request/response
  inspection, secret redaction, per-host settings, and storage retention.
- [NAS storage](nas-storage.md) — keep the record on the NAS over Tailscale: local
  primary with an append-only replica of the log, shaped to port to a cloud store.
- [Session handles](session-handles.md) — short session ids and a `last` word, so a
  session can be read from the terminal without its full id.
