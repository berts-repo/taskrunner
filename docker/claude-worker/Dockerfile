FROM node:20-slim

RUN npm install -g @anthropic-ai/claude-code@2.1.142

RUN useradd --create-home --shell /bin/sh worker

USER worker

ENV HOME=/home/worker

WORKDIR /workspace

CMD ["claude", "--version"]
