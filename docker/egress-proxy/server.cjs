// Egress proxy sidecar (PLAN § Container posture). The worker container's
// only route out is this HTTP proxy; it forwards CONNECT tunnels and plain
// HTTP requests whose destination host matches the allowlist, and prints one
// JSON line per decision so the daemon can fold them into the audit log.
//
// The allowlist names hosts, but DNS is attacker-influencable: an approved
// domain can resolve (or rebind mid-turn) to loopback, the LAN, or the
// Docker host. So the proxy resolves the name itself, refuses special-use
// destination addresses, and connects to the exact address it vetted.
// Deliberate local destinations remain expressible two ways: an IP-literal
// allowlist entry ("127.0.0.1:8080") or the Docker-provided host names
// ("host.docker.internal:11434"), both of which the operator wrote out
// explicitly. Entries without an explicit port allow only 80 and 443.
//
// No dependencies on purpose: the image is just node + this file.

"use strict";

const dns = require("node:dns");
const http = require("node:http");
const net = require("node:net");

// PORT=0 lets tests bind an ephemeral port; the container always uses 3128.
const PORT = process.env.PORT === undefined ? 3128 : Number(process.env.PORT);

/** @type {string[]} entries like "api.openai.com", "*.chatgpt.com", "host.docker.internal:11434" */
const allowed = JSON.parse(process.env.TASKRUNNER_ALLOWED_DOMAINS || "[]");

// Names Docker's embedded DNS resolves to the host gateway; a task cannot
// rebind these, so their (private) addresses are trusted when allowlisted.
const DOCKER_HOST_NAMES = new Set(["host.docker.internal", "gateway.docker.internal"]);

function log(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

/** Splits an allowlist entry into its host and optional port. */
function parseEntry(entry) {
  const colon = entry.lastIndexOf(":");
  if (colon !== -1 && /^\d+$/.test(entry.slice(colon + 1))) {
    return { host: entry.slice(0, colon), port: Number(entry.slice(colon + 1)) };
  }
  return { host: entry, port: null };
}

/** Matches host and port against one allowlist entry. */
function entryMatches(entry, host, port) {
  const { host: entryHost, port: entryPort } = parseEntry(entry);
  // Portless entries cover only the standard web ports; anything else must
  // be spelled out, so an approved domain is not an open door to any port.
  if (entryPort === null ? port !== 80 && port !== 443 : entryPort !== port) return false;
  if (entryHost.startsWith("*.")) {
    return host.endsWith(entryHost.slice(1)); // ".domain" suffix, subdomains only
  }
  return host === entryHost;
}

/** True for addresses that must never be reached via a DNS name: loopback,
 * private, link-local, CGNAT, multicast, and other special-use ranges. */
function isSpecialAddress(ip) {
  if (net.isIPv4(ip)) {
    const [a, b] = ip.split(".").map(Number);
    return (
      a === 0 ||
      a === 10 ||
      a === 127 ||
      (a === 100 && b >= 64 && b <= 127) ||
      (a === 169 && b === 254) ||
      (a === 172 && b >= 16 && b <= 31) ||
      (a === 192 && b === 0) ||
      (a === 192 && b === 168) ||
      (a === 198 && (b === 18 || b === 19)) ||
      ip.startsWith("198.51.100.") ||
      ip.startsWith("203.0.113.") ||
      a >= 224
    );
  }
  const v6 = ip.toLowerCase();
  const mapped = v6.match(/^::ffff:(\d+\.\d+\.\d+\.\d+)$/);
  if (mapped) return isSpecialAddress(mapped[1]);
  return (
    v6 === "::" ||
    v6 === "::1" ||
    v6.startsWith("fc") || // ULA fc00::/7
    v6.startsWith("fd") ||
    v6.startsWith("ff") || // multicast ff00::/8
    /^fe[89ab]/.test(v6) || // link-local fe80::/10
    v6.startsWith("2001:db8:")
  );
}

/**
 * Full decision for one request: allowlist policy, then destination-address
 * vetting. Resolves the name once and returns the vetted address so callers
 * connect to exactly what was checked (no second, rebindable lookup).
 * Logs one decision line per request.
 *
 * @returns {Promise<{ok: true, address: string} | {ok: false, status: number}>}
 */
async function decide(host, port) {
  const entry = allowed.find((e) => entryMatches(e, host, port));
  if (!entry) {
    log({ egress: "refused", host, port, reason: "allowlist" });
    return { ok: false, status: 403 };
  }
  const entryHost = parseEntry(entry).host;

  if (net.isIP(host)) {
    // An IP-literal entry is the operator pinning this exact address; only
    // then may it be special-use.
    if (entryHost !== host && isSpecialAddress(host)) {
      log({ egress: "refused", host, port, reason: "special-address" });
      return { ok: false, status: 403 };
    }
    log({ egress: "allowed", host, port });
    return { ok: true, address: host };
  }

  let addresses;
  try {
    addresses = await dns.promises.lookup(host, { all: true });
  } catch {
    // Name does not resolve: policy allowed it, the connection just fails.
    log({ egress: "allowed", host, port });
    return { ok: false, status: 502 };
  }
  const hostPinned = DOCKER_HOST_NAMES.has(host) && entryHost === host;
  if (!hostPinned && addresses.some((a) => isSpecialAddress(a.address))) {
    log({ egress: "refused", host, port, reason: "special-address" });
    return { ok: false, status: 403 };
  }
  log({ egress: "allowed", host, port });
  // Prefer IPv4: the sidecar's outward network frequently has no v6 route,
  // and a pinned unreachable v6 address would hang the tunnel.
  const pick = addresses.find((a) => a.family === 4) ?? addresses[0];
  return { ok: true, address: pick.address };
}

const server = http.createServer(async (req, res) => {
  // Plain HTTP proxying: request-target is an absolute URI.
  let url;
  try {
    url = new URL(req.url);
  } catch {
    res.writeHead(400);
    res.end("proxy: absolute-form request-target required\n");
    return;
  }
  // URL.hostname keeps IPv6 brackets; strip them for net/dns APIs.
  const host = url.hostname.replace(/^\[|\]$/g, "");
  const port = Number(url.port) || 80;
  const decision = await decide(host, port);
  if (!decision.ok) {
    res.writeHead(decision.status);
    res.end(decision.status === 403 ? "egress refused by taskrunner allowlist\n" : "");
    return;
  }
  const upstream = http.request(
    // Connect to the vetted address; the Host header still names the domain.
    { host: decision.address, port, method: req.method, path: url.pathname + url.search, headers: req.headers },
    (upRes) => {
      res.writeHead(upRes.statusCode || 502, upRes.headers);
      upRes.pipe(res);
    },
  );
  upstream.on("error", () => {
    if (!res.headersSent) res.writeHead(502);
    res.end();
  });
  req.on("error", () => upstream.destroy());
  req.pipe(upstream);
});

/** Splits a CONNECT target, tolerating bracketed IPv6 literals. */
function parseConnectTarget(target) {
  const match = /^(?:\[([^\]]+)\]|([^:]+))(?::(\d+))?$/.exec(String(target));
  if (!match) return null;
  return { host: match[1] ?? match[2], port: match[3] ? Number(match[3]) : 443 };
}

// HTTPS (and any TCP) goes through CONNECT tunnels.
server.on("connect", async (req, clientSocket, head) => {
  // The socket is ours the moment the event fires; without a handler a
  // client reset (even on a refused or still-resolving request) is an
  // unhandled 'error' event that takes down the whole sidecar.
  clientSocket.on("error", () => clientSocket.destroy());
  const target = parseConnectTarget(req.url);
  const decision = target ? await decide(target.host, target.port) : { ok: false, status: 400 };
  if (!decision.ok) {
    clientSocket.write(`HTTP/1.1 ${decision.status === 403 ? "403 Forbidden" : "502 Bad Gateway"}\r\n\r\n`);
    clientSocket.end();
    return;
  }
  const upstream = net.connect(target.port, decision.address, () => {
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
