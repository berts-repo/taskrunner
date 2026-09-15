# Revisit

Things to come back to or possibly remove. Not a build queue.

- **`docs/test-anchor.txt`** · a log fingerprint committed in cd160a1 that no doc
  explains; unclear whether it is a test fixture or a personal anchor · when the owner
  says which: move it to `tests/fixtures/` and name the test that uses it, or keep it
  and say why in [Keeping your own anchor](../guide/log-integrity.md#keeping-your-own-anchor)
  · `docs/test-anchor.txt`
- **Codex report's HTML twin** · `docs/security/2026-09-13-codex-three-blocked-connections.html`
  (266 KB) is rendered by a script, so its text couldn't be compared with the Markdown
  report · decide whether it duplicates the Markdown (delete it) or is the version to
  keep · `docs/security/`
- **Older shipped work has no archive packages** · work before 2026-09-13 is recorded
  only in git history · add a retroactive entry when a current doc needs to point at
  it · `git log`
