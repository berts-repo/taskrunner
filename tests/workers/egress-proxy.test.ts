import { spawn, type ChildProcess } from "node:child_process";
import * as http from "node:http";
import * as net from "node:net";
import { join } from "node:path";
import { createInterface } from "node:readline";
import type { Readable } from "node:stream";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

// Exercises the real proxy script the sidecar image runs (no Docker needed:
// it is plain node). Allowed upstream traffic goes to a local HTTP server;
// refused destinations never see a connection.

const SERVER_JS = join(__dirname, "../../docker/egress-proxy/server.cjs");

let proxy: ChildProcess;
let proxyPort: number;
let upstream: http.Server;
let upstreamPort: number;
const decisions: { egress: string; host: string; port: number; reason?: string }[] = [];

beforeAll(async () => {
  upstream = http.createServer((_req, res) => res.end("upstream says hi"));
  await new Promise<void>((resolve) => upstream.listen(0, "127.0.0.1", resolve));
  upstreamPort = (upstream.address() as net.AddressInfo).port;

  proxy = spawn(process.execPath, [SERVER_JS], {
    env: {
      ...process.env,
      PORT: "0",
      TASKRUNNER_ALLOWED_DOMAINS: JSON.stringify([
        // Loopback is reachable only because the operator pinned the exact
        // IP literal and port; a portless "127.0.0.1" would cover 80/443 only.
        `127.0.0.1:${upstreamPort}`,
        "*.allowed.test",
        `scoped.test:${upstreamPort}`,
        `localhost:${upstreamPort}`,
      ]),
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const rl = createInterface({ input: proxy.stdout as Readable });
  proxyPort = await new Promise<number>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("proxy did not start")), 5000);
    rl.on("line", (line) => {
      const obj = JSON.parse(line) as Record<string, unknown>;
      if (obj["proxy"] === "listening") {
        clearTimeout(timer);
        resolve(Number(obj["port"]));
      } else if (typeof obj["egress"] === "string") {
        decisions.push(obj as (typeof decisions)[number]);
      }
    });
  });
});

afterAll(async () => {
  proxy.kill("SIGTERM");
  await new Promise<void>((resolve) => upstream.close(() => resolve()));
});

/**
 * Sends a CONNECT and resolves with the proxy's status line, or "closed"
 * when the proxy drops the socket without replying (allowed host whose
 * upstream connection failed).
 */
function connectStatus(target: string): Promise<{ status: string; socket: net.Socket }> {
  return new Promise((resolve, reject) => {
    const socket = net.connect(proxyPort, "127.0.0.1", () => {
      socket.write(`CONNECT ${target} HTTP/1.1\r\nHost: ${target}\r\n\r\n`);
    });
    let settled = false;
    socket.once("data", (data) => {
      settled = true;
      resolve({ status: data.toString("utf8").split("\r\n")[0]!, socket });
    });
    socket.on("close", () => {
      if (!settled) resolve({ status: "closed", socket });
    });
    socket.on("error", (err) => {
      if (!settled) reject(err);
    });
  });
}

async function waitForDecision(host: string): Promise<{ egress: string; port: number }> {
  for (let i = 0; i < 100; i++) {
    const hit = decisions.find((d) => d.host === host);
    if (hit) return hit;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  throw new Error(`no egress decision logged for ${host}`);
}

describe("egress proxy", () => {
  it("tunnels CONNECT to an allowlisted host and logs the decision", async () => {
    const { status, socket } = await connectStatus(`127.0.0.1:${upstreamPort}`);
    expect(status).toBe("HTTP/1.1 200 Connection Established");

    const body = await new Promise<string>((resolve) => {
      let buf = "";
      socket.on("data", (d) => {
        buf += d.toString("utf8");
        if (buf.includes("upstream says hi")) resolve(buf);
      });
      socket.write(`GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n`);
    });
    expect(body).toContain("upstream says hi");
    socket.destroy();
    expect(await waitForDecision("127.0.0.1")).toMatchObject({ egress: "allowed" });
  });

  it("refuses CONNECT to hosts off the allowlist with 403", async () => {
    const { status, socket } = await connectStatus("evil.test:443");
    expect(status).toBe("HTTP/1.1 403 Forbidden");
    socket.destroy();
    expect(await waitForDecision("evil.test")).toMatchObject({
      egress: "refused",
      port: 443,
      reason: "allowlist",
    });
  });

  it("limits portless allowlist entries to ports 80 and 443", async () => {
    const { status, socket } = await connectStatus("other.allowed.test:8080");
    expect(status).toBe("HTTP/1.1 403 Forbidden");
    socket.destroy();
    expect(await waitForDecision("other.allowed.test")).toMatchObject({
      egress: "refused",
      port: 8080,
      reason: "allowlist",
    });
  });

  it("refuses allowlisted names that resolve to special-use addresses", async () => {
    // localhost is on the allowlist with the right port, but it resolves to
    // loopback — only an IP-literal entry may point at special-use space.
    const { status, socket } = await connectStatus(`localhost:${upstreamPort}`);
    expect(status).toBe("HTTP/1.1 403 Forbidden");
    socket.destroy();
    expect(await waitForDecision("localhost")).toMatchObject({
      egress: "refused",
      reason: "special-address",
    });
  });

  it("matches *.wildcard entries against subdomains only", async () => {
    const sub = await connectStatus("api.allowed.test:443");
    // Allowed by policy; the upstream connect itself fails (no such DNS name),
    // which surfaces as a dropped socket, not a 403.
    sub.socket.destroy();
    expect(await waitForDecision("api.allowed.test")).toMatchObject({ egress: "allowed" });

    const apex = await connectStatus("allowed.test:443");
    expect(apex.status).toBe("HTTP/1.1 403 Forbidden");
    apex.socket.destroy();
    expect(await waitForDecision("allowed.test")).toMatchObject({ egress: "refused" });
  });

  it("honors :port scoping in allowlist entries", async () => {
    const wrongPort = await connectStatus(`scoped.test:${upstreamPort + 1}`);
    expect(wrongPort.status).toBe("HTTP/1.1 403 Forbidden");
    wrongPort.socket.destroy();

    const rightPort = await connectStatus(`scoped.test:${upstreamPort}`);
    rightPort.socket.destroy();
    const hits = decisions.filter((d) => d.host === "scoped.test");
    expect(hits.map((d) => d.egress)).toEqual(["refused", "allowed"]);
  });

  it("survives a client that resets the connection mid-request", async () => {
    // A refused CONNECT whose client sends RST instead of FIN used to be an
    // unhandled 'error' event that took the whole sidecar down.
    await new Promise<void>((resolve, reject) => {
      const socket = net.connect(proxyPort, "127.0.0.1", () => {
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
    expect(proxy.exitCode).toBeNull();

    // And it still serves the next request.
    const { status, socket } = await connectStatus(`127.0.0.1:${upstreamPort}`);
    expect(status).toBe("HTTP/1.1 200 Connection Established");
    socket.destroy();
  });

  it("proxies plain HTTP absolute-form requests through the allowlist", async () => {
    const allowed = await new Promise<{ status: number; body: string }>((resolve, reject) => {
      const req = http.request(
        {
          host: "127.0.0.1",
          port: proxyPort,
          method: "GET",
          path: `http://127.0.0.1:${upstreamPort}/`,
        },
        (res) => {
          let body = "";
          res.on("data", (d) => (body += d));
          res.on("end", () => resolve({ status: res.statusCode ?? 0, body }));
        },
      );
      req.on("error", reject);
      req.end();
    });
    expect(allowed.status).toBe(200);
    expect(allowed.body).toBe("upstream says hi");

    const refused = await new Promise<number>((resolve, reject) => {
      const req = http.request(
        { host: "127.0.0.1", port: proxyPort, method: "GET", path: "http://evil.test/" },
        (res) => {
          res.resume();
          resolve(res.statusCode ?? 0);
        },
      );
      req.on("error", reject);
      req.end();
    });
    expect(refused).toBe(403);
  });
});
