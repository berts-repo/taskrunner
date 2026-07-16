// Egress proxy sidecar (PLAN § Container posture). The worker container's
// only route out is this HTTP proxy; it forwards CONNECT tunnels and plain
// HTTP requests whose destination host matches the allowlist, and prints one
// JSON line per decision so the daemon can fold them into the audit log.
//
// No dependencies on purpose: the image is just node + this file.

"use strict";

const http = require("node:http");
const net = require("node:net");

// PORT=0 lets tests bind an ephemeral port; the container always uses 3128.
const PORT = process.env.PORT === undefined ? 3128 : Number(process.env.PORT);

/** @type {string[]} entries like "api.openai.com", "*.chatgpt.com", "host.docker.internal:11434" */
const allowed = JSON.parse(process.env.TASKRUNNER_ALLOWED_DOMAINS || "[]");

function log(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

/** Matches host (and optionally port) against one allowlist entry. */
function entryMatches(entry, host, port) {
  let entryHost = entry;
  let entryPort = null;
  const colon = entry.lastIndexOf(":");
  if (colon !== -1 && /^\d+$/.test(entry.slice(colon + 1))) {
    entryHost = entry.slice(0, colon);
    entryPort = Number(entry.slice(colon + 1));
  }
  if (entryPort !== null && entryPort !== port) return false;
  if (entryHost.startsWith("*.")) {
    return host.endsWith(entryHost.slice(1)); // ".domain" suffix, subdomains only
  }
  return host === entryHost;
}

function isAllowed(host, port) {
  return allowed.some((entry) => entryMatches(entry, host, port));
}

function decide(host, port) {
  const ok = isAllowed(host, port);
  log({ egress: ok ? "allowed" : "refused", host, port });
  return ok;
}

const server = http.createServer((req, res) => {
  // Plain HTTP proxying: request-target is an absolute URI.
  let url;
  try {
    url = new URL(req.url);
  } catch {
    res.writeHead(400);
    res.end("proxy: absolute-form request-target required\n");
    return;
  }
  const port = Number(url.port) || 80;
  if (!decide(url.hostname, port)) {
    res.writeHead(403);
    res.end("egress refused by taskrunner allowlist\n");
    return;
  }
  const upstream = http.request(
    { host: url.hostname, port, method: req.method, path: url.pathname + url.search, headers: req.headers },
    (upRes) => {
      res.writeHead(upRes.statusCode || 502, upRes.headers);
      upRes.pipe(res);
    },
  );
  upstream.on("error", () => {
    if (!res.headersSent) res.writeHead(502);
    res.end();
  });
  req.pipe(upstream);
});

// HTTPS (and any TCP) goes through CONNECT tunnels.
server.on("connect", (req, clientSocket, head) => {
  const [host, portStr] = String(req.url).split(":");
  const port = Number(portStr) || 443;
  if (!host || !decide(host, port)) {
    clientSocket.write("HTTP/1.1 403 Forbidden\r\n\r\n");
    clientSocket.end();
    return;
  }
  const upstream = net.connect(port, host, () => {
    clientSocket.write("HTTP/1.1 200 Connection Established\r\n\r\n");
    if (head && head.length) upstream.write(head);
    upstream.pipe(clientSocket);
    clientSocket.pipe(upstream);
  });
  upstream.on("error", () => clientSocket.destroy());
  clientSocket.on("error", () => upstream.destroy());
});

server.listen(PORT, () => {
  log({ proxy: "listening", port: server.address().port, allowed });
});
