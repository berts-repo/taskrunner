# CLAUDE.md

## Teaching objective

The person building this project is learning as they go. When a design touches a
mechanism used in real-world systems, explain it — briefly, plainly, from the
user's perspective — and say how companies use the same mechanism in practice.
Do this as the topic comes up, not as a separate lecture.

Topics that are known to matter here:

- **TLS-inspection / "sit in the middle" proxies.** How HTTPS encrypts a
  conversation in transit, why a recorder must terminate TLS with its own
  certificate to see request bodies, why the client has to trust that
  certificate (`HTTPS_PROXY` + `NODE_EXTRA_CA_CERTS` for Claude Code, system
  trust store for Codex), and why OAuth vs API-key auth makes no difference.
  Companies use exactly this for employee-traffic inspection, DLP, and audit
  gateways; Anthropic documents it as the supported enterprise-proxy path.
- **Audit by ingestion vs audit at the wire.** Reading the files a harness
  writes (cheap, incomplete, needs a parser per harness) versus capturing at
  the network (complete by construction, needs the certificate). Why the
  Docker workers get the wire option for free — taskrunner already owns the
  egress proxy and the image — while host sessions need a machine-level change.
- **One archive, many working copies.** A harness's own store (Hermes
  `state.db`, Claude's `.jsonl`, Codex's sessions) is operational; the
  taskrunner archive is the audit. Two copies is fine; two sources of truth
  is not.

Keep explanations short. Weigh options with ✅/❌ from the user's seat. Recommend
one. Don't build unless asked.

## Working practices

Clean and readable is a goal, not a nicety. Remove old code in the same change that
replaces it — no parallel old-and-new. Prefer the obvious way over the clever one;
comments say *why*, not *what*. Every behaviour change updates the doc that describes
it in the same commit. See `docs/proposals/complete-audit.md` § How the work is done.
