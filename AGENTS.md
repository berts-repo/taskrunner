# Agent rules

Read `CONTEXT.md` first. Read `LIBRARIAN.md` before any documentation work.

## How the work is done

Clean and readable is a goal, not a nicety after it.

- **Remove old code as it is replaced.** When a path is superseded, delete it in the
  same change. No parallel old-and-new. Legacy event kinds stay parseable only
  because the log is append-only; mark them as such and keep the note short.
- **Simplify for a human reader.** Prefer one obvious way over a clever one. A
  function should be readable top to bottom by someone new to the project; if it
  needs a paragraph of comment to explain *what* it does, restructure it instead.
  Comments say *why*.
- **Docs move with the code.** Every change that alters behaviour updates, in the
  same commit, the docs that describe it: `docs/reference/` for how it works, the
  affected `docs/guide/` page for what the user sees, `docs/security/overview.md` when
  the security model changes, and `README.md` when the guide index changes. A doc
  that lags the code is worse than none: it is confidently wrong.

## After a merge

When a branch merges into `main`, or a work package in `docs/work/active/` closes,
offer a documentation pass over what changed: suggest handing it to the `luna` worker
under the rules in `LIBRARIAN.md`, then wait for a yes. A pass spends the user's Codex
quota and comes back as a branch to review, so never start one unasked. Skip the
offer when the merge changed neither behaviour nor docs.
