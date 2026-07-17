---
name: worker-login
description: Sign a taskrunner Docker worker (codex or claude) into its auth volume — use when a worker errors with a missing/expired login or a new machine needs worker credentials.
---

# Worker sign-in

Each Docker worker authenticates from its own named volume
(`taskrunner-codex-home`, `taskrunner-claude-home`). Never mount or copy
host credentials (`~/.codex`, `~/.claude`): host and container sessions
sharing one refresh token invalidate each other (`refresh_token_reused`).

The login itself is the user's action — set up the exact command, hand it
to them, and verify afterwards. Do not run the login container yourself.

## Mount paths must match the daemon's

The daemon mounts `taskrunner-codex-home` at `/home/worker/.codex` (codex
keeps everything under `~/.codex`) and `taskrunner-claude-home` at
`/home/worker` (claude spreads state across the home directory). Log in at
those SAME paths. Logging codex in with the volume at `/home/worker` buries
auth.json at `<volume>/.codex/auth.json` where the daemon's mount cannot see
it — turns then fail with 401 "Missing bearer or basic authentication"
against `wss://api.openai.com/v1/responses` (unauthenticated codex falls
back to API-key mode).

## Steps

1. Ensure images exist: `docker image inspect taskrunner/codex-worker`
   (build with `npm run build:images` if missing).
2. Give the user the command for the worker in question, to run in a real
   terminal (`-it` fails under the `!` session prefix — no TTY):

   Codex — MUST use device auth. The default `codex login` starts a
   localhost callback server bound to the container's loopback, which no
   `-p` publish can reach (browser gets ERR_EMPTY_RESPONSE):

       docker run -it --rm -v taskrunner-codex-home:/home/worker/.codex taskrunner/codex-worker codex login --device-auth

   Claude — interactive login prints a URL and accepts a pasted code:

       docker run -it --rm -v taskrunner-claude-home:/home/worker taskrunner/claude-worker claude /login

3. The user opens the printed URL in their browser and approves; the
   credentials persist in the volume.
4. Verify without touching the credentials (same mounts as the daemon):
   - codex: `docker run --rm -v taskrunner-codex-home:/home/worker/.codex taskrunner/codex-worker codex login status`
     → expect "Logged in using ChatGPT"
   - claude: `docker run --rm -v taskrunner-claude-home:/home/worker alpine ls -la /home/worker/.claude`
     → expect `.credentials.json` in the listing (`-a` is required — plain
     `ls` hides it, making a logged-in volume look logged-out)

Host networking is not a fallback on this machine: Docker Desktop here does
not forward host-network containers to the Mac's localhost (verified
2026-07-16). With ChatGPT auth, a healthy codex turn talks to `chatgpt.com`
and `ab.chatgpt.com` only — codex hitting `api.openai.com` is itself a sign
the credentials were not found.
