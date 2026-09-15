# CLAUDE.md

## Teaching objective

The person building this project is learning as they go. When a design touches a mechanism used in real-world systems, explain it — briefly, plainly, from the user's perspective — and say how companies use the same mechanism in practice.

Do this as the topic comes up, not as a separate lecture.

Keep explanations short. Weigh options with ✅/❌ from the user's seat. Recommend
one.

Don't build unless asked.

## Working practices

Clean and readable is a goal, not a nicety. Remove old code in the same change that
replaces it — no parallel old-and-new. Prefer the obvious way over the clever one;
comments say *why*, not *what*. Every behaviour change updates the doc that describes
it in the same commit. See `docs/proposals/complete-audit.md` § How the work is done.
