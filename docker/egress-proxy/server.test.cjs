// Tests for the proxy script the sidecar image runs. It is plain node, so no
// Docker is needed: allowed upstream traffic goes to a local HTTP server, and
// refused destinations never see a connection. Node's own test runner keeps the
// image's "no dependencies" promise true for its tests too.
//
// Run: node --test docker/egress-proxy/server.test.cjs (cargo test runs it as well).
"use strict";

const assert = require("node:assert/strict");
const { spawn } = require("node:child_process");
const http = require("node:http");
const net = require("node:net");
const path = require("node:path");
const { createInterface } = require("node:readline");
const { after, before, describe, it } = require("node:test");

const SERVER_JS = path.join(__dirname, "server.cjs");

// Requiring the script does not start the listener (guarded on require.main).
const { isSpecialAddress } = require(SERVER_JS);

/** Starts a proxy with this allowlist; resolves once it reports its port. */
function startProxy(allowedDomains) {
  const child = spawn(process.execPath, [SERVER_JS], {
    env: { ...process.env, PORT: "0", TASKRUNNER_ALLOWED_DOMAINS: JSON.stringify(allowedDomains) },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const decisions = [];
  const lines = createInterface({ input: child.stdout });
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("proxy did not start")), 5000);
    lines.on("line", (line) => {
      const obj = JSON.parse(line);
      if (obj.proxy === "listening") {
        clearTimeout(timer);
        resolve({ child, port: Number(obj.port), decisions });
      } else if (typeof obj.egress === "string") {
        decisions.push(obj);
      }
    });
  });
}

/**
 * Sends a CONNECT and resolves with the proxy's status line, or "closed"
 * when the proxy drops the socket without replying (allowed host whose
 * upstream connection failed).
 */
function connectStatus(target, port) {
  return new Promise((resolve, reject) => {
    const socket = net.connect(port, "127.0.0.1", () => {
      socket.write(`CONNECT ${target} HTTP/1.1\r\nHost: ${target}\r\n\r\n`);
    });
    let settled = false;
    socket.once("data", (data) => {
      settled = true;
      resolve({ status: data.toString("utf8").split("\r\n")[0], socket });
    });
    socket.on("close", () => {
      if (!settled) resolve({ status: "closed", socket });
    });
    socket.on("error", (err) => {
      if (!settled) reject(err);
    });
  });
}

async function waitForDecision(host, decisions) {
  for (let i = 0; i < 100; i++) {
    const hit = decisions.find((d) => d.host === host);
    if (hit) return hit;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  throw new Error(`no egress decision logged for ${host}`);
}

/** Asserts each expected field; fields not named are not checked. */
function assertFields(actual, expected) {
  for (const [key, value] of Object.entries(expected)) {
    assert.equal(actual[key], value, `${key} in ${JSON.stringify(actual)}`);
  }
}

describe("egress proxy", () => {
  let proxy;
  let upstream;
  let upstreamPort;

  before(async () => {
    upstream = http.createServer((_req, res) => res.end("upstream says hi"));
    await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
    upstreamPort = upstream.address().port;
    proxy = await startProxy([
      // Loopback is reachable only because the operator pinned the exact
      // IP literal and port; a portless "127.0.0.1" would cover 80/443 only.
      `127.0.0.1:${upstreamPort}`,
      "*.allowed.test",
      `scoped.test:${upstreamPort}`,
      `localhost:${upstreamPort}`,
    ]);
  });

  after(async () => {
    proxy.child.kill("SIGTERM");
    await new Promise((resolve) => upstream.close(resolve));
  });

  it("tunnels CONNECT to an allowlisted host and logs the decision", async () => {
    const { status, socket } = await connectStatus(`127.0.0.1:${upstreamPort}`, proxy.port);
    assert.equal(status, "HTTP/1.1 200 Connection Established");

    const body = await new Promise((resolve) => {
      let buf = "";
      socket.on("data", (d) => {
        buf += d.toString("utf8");
        if (buf.includes("upstream says hi")) resolve(buf);
      });
      socket.write("GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    });
    assert.match(body, /upstream says hi/);
    socket.destroy();
    assertFields(await waitForDecision("127.0.0.1", proxy.decisions), { egress: "allowed" });
  });

  it("refuses CONNECT to hosts off the allowlist with 403", async () => {
    const { status, socket } = await connectStatus("evil.test:443", proxy.port);
    assert.equal(status, "HTTP/1.1 403 Forbidden");
    socket.destroy();
    assertFields(await waitForDecision("evil.test", proxy.decisions), {
      egress: "refused",
      port: 443,
      reason: "allowlist",
    });
  });

  it("limits portless allowlist entries to ports 80 and 443", async () => {
    const { status, socket } = await connectStatus("other.allowed.test:8080", proxy.port);
    assert.equal(status, "HTTP/1.1 403 Forbidden");
    socket.destroy();
    assertFields(await waitForDecision("other.allowed.test", proxy.decisions), {
      egress: "refused",
      port: 8080,
      reason: "allowlist",
    });
  });

  it("refuses allowlisted names that resolve to special-use addresses", async () => {
    // localhost is on the allowlist with the right port, but it resolves to
    // loopback — only an IP-literal entry may point at special-use space.
    const { status, socket } = await connectStatus(`localhost:${upstreamPort}`, proxy.port);
    assert.equal(status, "HTTP/1.1 403 Forbidden");
    socket.destroy();
    assertFields(await waitForDecision("localhost", proxy.decisions), {
      egress: "refused",
      reason: "special-address",
    });
  });

  it("matches *.wildcard entries against subdomains only", async () => {
    const sub = await connectStatus("api.allowed.test:443", proxy.port);
    // Allowed by policy; the upstream connect itself fails (no such DNS name),
    // which surfaces as a dropped socket, not a 403.
    sub.socket.destroy();
    assertFields(await waitForDecision("api.allowed.test", proxy.decisions), { egress: "allowed" });

    const apex = await connectStatus("allowed.test:443", proxy.port);
    assert.equal(apex.status, "HTTP/1.1 403 Forbidden");
    apex.socket.destroy();
    assertFields(await waitForDecision("allowed.test", proxy.decisions), { egress: "refused" });
  });

  it("honors :port scoping in allowlist entries", async () => {
    const wrongPort = await connectStatus(`scoped.test:${upstreamPort + 1}`, proxy.port);
    assert.equal(wrongPort.status, "HTTP/1.1 403 Forbidden");
    wrongPort.socket.destroy();

    const rightPort = await connectStatus(`scoped.test:${upstreamPort}`, proxy.port);
    rightPort.socket.destroy();
    const hits = proxy.decisions.filter((d) => d.host === "scoped.test");
    assert.deepEqual(
      hits.map((d) => d.egress),
      ["refused", "allowed"],
    );
  });

  it("survives a client that resets the connection mid-request", async () => {
    // A refused CONNECT whose client sends RST instead of FIN used to be an
    // unhandled 'error' event that took the whole sidecar down.
    await new Promise((resolve, reject) => {
      const socket = net.connect(proxy.port, "127.0.0.1", () => {
        socket.write("CONNECT evil.test:443 HTTP/1.1\r\nHost: evil.test:443\r\n\r\n");
        setTimeout(() => {
          socket.resetAndDestroy();
          resolve();
        }, 50);
      });
      socket.on("error", () => {});
      socket.setTimeout(5000, () => reject(new Error("proxy did not respond")));
    });
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(proxy.child.exitCode, null);

    // And it still serves the next request.
    const { status, socket } = await connectStatus(`127.0.0.1:${upstreamPort}`, proxy.port);
    assert.equal(status, "HTTP/1.1 200 Connection Established");
    socket.destroy();
  });

  describe("with a bare * allowlist entry", () => {
    let wildcard;

    before(async () => {
      wildcard = await startProxy(["*"]);
    });

    after(() => {
      wildcard.child.kill("SIGTERM");
    });

    it("allows any public hostname on the web ports", async () => {
      // Policy allows it; the upstream connect fails (no such DNS name),
      // which surfaces as a dropped socket, not a 403.
      const { socket } = await connectStatus("anything.example:443", wildcard.port);
      socket.destroy();
      assertFields(await waitForDecision("anything.example", wildcard.decisions), {
        egress: "allowed",
      });
    });

    it("still limits the portless * to ports 80 and 443", async () => {
      const { status, socket } = await connectStatus("odd-port.example:8080", wildcard.port);
      assert.equal(status, "HTTP/1.1 403 Forbidden");
      socket.destroy();
      assertFields(await waitForDecision("odd-port.example", wildcard.decisions), {
        egress: "refused",
        port: 8080,
        reason: "allowlist",
      });
    });

    it("still refuses names resolving to special-use addresses", async () => {
      const { status, socket } = await connectStatus("localhost:443", wildcard.port);
      assert.equal(status, "HTTP/1.1 403 Forbidden");
      socket.destroy();
      assertFields(await waitForDecision("localhost", wildcard.decisions), {
        egress: "refused",
        reason: "special-address",
      });
    });

    it("still refuses special-use IP literals", async () => {
      const { status, socket } = await connectStatus("192.168.1.1:443", wildcard.port);
      assert.equal(status, "HTTP/1.1 403 Forbidden");
      socket.destroy();
      assertFields(await waitForDecision("192.168.1.1", wildcard.decisions), {
        egress: "refused",
        reason: "special-address",
      });
    });
  });

  it("proxies plain HTTP absolute-form requests through the allowlist", async () => {
    const allowed = await new Promise((resolve, reject) => {
      const req = http.request(
        { host: "127.0.0.1", port: proxy.port, method: "GET", path: `http://127.0.0.1:${upstreamPort}/` },
        (res) => {
          let body = "";
          res.on("data", (d) => (body += d));
          res.on("end", () => resolve({ status: res.statusCode, body }));
        },
      );
      req.on("error", reject);
      req.end();
    });
    assert.equal(allowed.status, 200);
    assert.equal(allowed.body, "upstream says hi");

    const refused = await new Promise((resolve, reject) => {
      const req = http.request(
        { host: "127.0.0.1", port: proxy.port, method: "GET", path: "http://evil.test/" },
        (res) => {
          res.resume();
          resolve(res.statusCode);
        },
      );
      req.on("error", reject);
      req.end();
    });
    assert.equal(refused, 403);
  });
});

// The private/loopback filter is the whole boundary when "*" egress is granted,
// so vet it directly: a fast unit test with no sockets.
describe("isSpecialAddress", () => {
  it("refuses IPv4 loopback and private ranges", () => {
    for (const ip of ["127.0.0.1", "10.0.0.5", "172.17.0.1", "192.168.1.1", "169.254.0.1", "0.0.0.0"]) {
      assert.equal(isSpecialAddress(ip), true, ip);
    }
  });

  it("refuses IPv6 loopback, ULA, link-local, and multicast", () => {
    for (const ip of ["::1", "::", "fd00::1", "fc00::1", "fe80::1", "ff02::1"]) {
      assert.equal(isSpecialAddress(ip), true, ip);
    }
  });

  it("refuses IPv4-mapped loopback in dotted form", () => {
    assert.equal(isSpecialAddress("::ffff:127.0.0.1"), true);
  });

  it("refuses IPv4-mapped addresses written in hex form", () => {
    assert.equal(isSpecialAddress("::ffff:7f00:1"), true); // 127.0.0.1
    assert.equal(isSpecialAddress("::ffff:ac11:1"), true); // 172.17.0.1 (Docker gateway)
    assert.equal(isSpecialAddress("::ffff:c0a8:101"), true); // 192.168.1.1
  });

  it("refuses NAT64-embedded private addresses", () => {
    assert.equal(isSpecialAddress("64:ff9b::7f00:1"), true); // 64:ff9b::/96 → 127.0.0.1
  });

  it("allows genuine public addresses, including hex-mapped public v4", () => {
    assert.equal(isSpecialAddress("1.1.1.1"), false);
    assert.equal(isSpecialAddress("8.8.8.8"), false);
    assert.equal(isSpecialAddress("::ffff:0808:0808"), false); // 8.8.8.8 mapped
    assert.equal(isSpecialAddress("2606:4700:4700::1111"), false);
  });
});
