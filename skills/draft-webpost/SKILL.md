---
name: draft-webpost
description: Draft or revise a short project portfolio post for ~/Projects/helloto using project evidence and relevant taskrunner history, and publish a reviewed draft when explicitly requested. Use for project webposts and blog drafts.
---

# Draft a project webpost

Create an unpublished MDX draft for the user's website at `~/Projects/helloto/`.
Read the site's applicable instructions and current post loader before writing.
Also inspect the article route and a representative post: the rendered page,
not older articles alone, determines heading and metadata conventions.
Use the current conversation to identify the source project; ask only if ambiguous.

## Editorial preferences

- First-person, plainspoken, technically useful, with some personality.
- Readers include hiring managers and technical interviewers, but the article is
  about the project. Do not discuss desired jobs, career direction, or what the
  project demonstrates to employers.
- Aim for 250–400 words unless the user asks for more. Make it easy to scan.
- Lead with what was built and what it does. Cover concrete completed work,
  one interesting implementation detail, and a result when evidence supports it.
  Use a short feature list when it helps. Avoid a fixed template when it feels forced.
- Do not turn a project summary into a long security architecture explanation.
  Let relevant security work speak through specifics; do not force a security angle.
- Avoid promotional claims, generic lessons, invented motivations or feelings,
  and boilerplate introductions or conclusions.
- Describe AI assistance honestly when relevant. Do not invent a division of labor
  or claim the user personally implemented or tested work the evidence assigns to
  an agent. Ask if specific attribution is necessary and unclear.

## Gather evidence

Start with this session, the source project's instructions, current code and docs,
and relevant diffs. Distinguish implemented behavior from plans and failed attempts.
Historical test results are historical; reading a test is not running it.

Check the source project's Git remotes and README for its GitHub repository.
When a public repository is available, include a normal Markdown link in the
article (for example, `[Source on GitHub](https://github.com/owner/repo)`).
Normalize SSH remotes to HTTPS and remove credentials from URLs. Verify which
repository represents the project when remotes differ; do not invent a URL or
assume a remote is public. If visibility cannot be established, record the link
as unresolved in `review.md` and ask before exposing a potentially private URL.

Use taskrunner's archive when earlier context would improve the article. Prefer
the available `search-transcripts` and `lookup-session` tools. Search by project
and specific topics, then read individual exchanges; do not load entire sessions
first. CLI equivalents:

```sh
taskrunner search 'specific topic' --project /absolute/project/path --limit 10
taskrunner sessions --project /absolute/project/path --limit 5
taskrunner session SESSION_ID --view outline
taskrunner session SESSION_ID --prompt EXCHANGE_NUMBER
```

Search hits contain session and exchange identifiers. For delegated work use
`lookup-task` or `taskrunner task TASK_ID --include transcript --prompt NUMBER`.
Query through these interfaces rather than opening or modifying SQLite directly.
If the archive is unavailable, continue with available evidence and mark gaps in
the private review note. Do not fabricate missing history.

## Find existing drafts

Before choosing a topic, inspect `~/Projects/helloto/.webpost-drafts/` (including
`review.md` files), the site's existing posts, and relevant Markdown notes in the
source project. Search by project name and article topic as well as filenames:
an older draft may be a dated writeup, not a file named "draft". Include hidden
and ignored files in these scoped searches (`rg --hidden --no-ignore`, excluding
build outputs, dependencies, and `.git`).

If the user refers to an earlier draft and local searches do not identify it,
search the archive for its topic, drafting requests, and saved paths; read the
matching exchanges. Include the website project or broaden the project filter
when needed. An empty folder or search result does not prove no draft exists.
Report the search scope and any remaining uncertainty rather than declaring
there is no draft. Verify a candidate's content and context before treating it
as the requested article; ask only if multiple plausible candidates remain.

Local posts are evidence of existing coverage, not proof of deployment.
Prefer current code for present behavior and session records for past decisions.
Keep credentials, private conversation excerpts, internal addresses, and personal
data out of the public article. Keep evidence pointers in the private review note.

## Continuing projects

Use these defaults, adapting when the user requests another approach:

- Continue a matching unfinished draft instead of making duplicates.
- For an existing project overview, propose a revised article when new work fits
  its scope. Preserve the existing published file; write a separate draft revision.
- A substantial new story can become a follow-up that links to the earlier post.
- Accumulate minor changes in the private review note until there is enough for a
  useful article. Say when the session does not justify a post.

These are editorial defaults; approval of writing style is not publishing approval.

## Save and review

Drafting alone does not authorize publishing. Keep drafts unpublished until the
user explicitly asks to publish the reviewed article. An explicit request such
as "post it" authorizes the publishing workflow below; do not ask again unless
the content or destination materially changes.

The canonical home for article drafts is
`~/Projects/helloto/.webpost-drafts/<slug>/`, outside the site's `posts/` and
`public/` directories. Project-local technical notes can supply evidence; do not
start a second article draft there or in Documents. When continuing an older draft
found elsewhere, save the continued version here and record its original path in
`review.md`; preserve the original unless its removal is authorized.
The current site hides `draft: true` posts even
from direct URLs, so that flag does not provide a rendered preview.

Write `post.mdx` using the site's frontmatter conventions: title, date, excerpt,
tags, and `draft: true`. The date is the drafting date, not a claimed publication
date. Use ordinary Markdown compatible with MDX and escape literal MDX syntax.
For a revision, record the intended original slug without overwriting it.

The current site reads `posts/<slug>.mdx` through `lib/posts.ts` and renders it
at `/webpost/<slug>` through `app/webpost/[slug]/page.tsx`. Recheck these files
when drafting. Use a quoted `YYYY-MM-DD` date, a short plain-text excerpt, and
a string array of tags, reusing relevant existing tags. The page renders the
title as its own H1, so start the body with prose and use H2 section headings;
do not repeat the title as an H1 even though some older posts do. Use ordinary
Markdown links for GitHub and `/webpost/<slug>` for related articles.
Record the intended `posts/<slug>.mdx` destination and `/webpost/<slug>` route
in `review.md` so the draft is ready for a later publishing workflow.

Alongside it maintain `review.md` with the source project, intended action
(new/revision/follow-up), related post paths, source revision and relevant session
exchanges, repository URL and visibility evidence (or why no source link is
included), verification limits, unresolved questions, and user feedback. This is
private drafting context and must not be copied into the public article. Read it
on subsequent invocations to avoid repeating coverage or losing edits. Do not
overwrite changes whose purpose is unclear; inspect the diff first.

Check frontmatter parsing and any relevant existing content checks without running
deployment scripts. Show the article in the conversation with a link to the saved
draft and briefly identify any material uncertainty. Invite focused feedback on
length, emphasis, and voice; apply feedback to the draft. Record durable preferences
in the review note. This skill is compiled into taskrunner; change its source at
`skills/draft-webpost/SKILL.md` in the taskrunner repository when updating shared
editorial guidance, not the generated copies or links installed for harnesses.

## Publish an approved draft

Inspect the site's Git status, remotes, current instructions, and deployment
configuration. Confirm the production branch from repository/deployment evidence;
do not assume it is named main. Bring it up to date without discarding local work.

Copy the approved article to its intended `posts/<slug>.mdx` path, remove
`draft: true` from that published copy, and use the publication date. Preserve
the private draft and review note outside published content and Git commits.
For an approved revision, inspect the current article before replacing it.
Run the site's relevant content/build checks and inspect the exact publishing
diff. Commit only the approved article and any necessary explicitly scoped assets.
Use the site's established deployment workflow; a push may itself deploy the site.
Do not force-push or include unrelated local changes.

Check deployment status and the live article URL. Record the commit, date, URL,
and verification result in `review.md`, and mark the draft published there so a
future invocation does not treat it as unfinished. Distinguish a successful push
from a verified live publication. If deployment fails, investigate within the
approved scope and report any remaining blocker accurately.
