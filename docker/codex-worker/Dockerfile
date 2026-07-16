FROM node:20-slim

RUN apt-get update && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g @openai/codex

RUN useradd --create-home --shell /bin/sh worker

USER worker

ENV HOME=/home/worker

WORKDIR /workspace

CMD ["codex", "--version"]
