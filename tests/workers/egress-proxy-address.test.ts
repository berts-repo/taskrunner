import { createRequire } from "node:module";
import { describe, expect, it } from "vitest";

// The proxy's private/loopback filter is the whole boundary when "*" egress is
// granted, so vet it directly. The pure helper is exported from the .cjs the
// sidecar runs; requiring it does not start the listener (guarded on
// require.main), so this stays a fast unit test with no Docker or sockets.
const require = createRequire(import.meta.url);
const { isSpecialAddress } = require("../../docker/egress-proxy/server.cjs") as {
  isSpecialAddress: (ip: string) => boolean;
};

describe("isSpecialAddress", () => {
  it("refuses IPv4 loopback and private ranges", () => {
    for (const ip of ["127.0.0.1", "10.0.0.5", "172.17.0.1", "192.168.1.1", "169.254.0.1", "0.0.0.0"]) {
      expect(isSpecialAddress(ip), ip).toBe(true);
    }
  });

  it("refuses IPv6 loopback, ULA, link-local, and multicast", () => {
    for (const ip of ["::1", "::", "fd00::1", "fc00::1", "fe80::1", "ff02::1"]) {
      expect(isSpecialAddress(ip), ip).toBe(true);
    }
  });

  it("refuses IPv4-mapped loopback in dotted form", () => {
    expect(isSpecialAddress("::ffff:127.0.0.1")).toBe(true);
  });

  it("refuses IPv4-mapped addresses written in hex form", () => {
    expect(isSpecialAddress("::ffff:7f00:1")).toBe(true); // 127.0.0.1
    expect(isSpecialAddress("::ffff:ac11:1")).toBe(true); // 172.17.0.1 (Docker gateway)
    expect(isSpecialAddress("::ffff:c0a8:101")).toBe(true); // 192.168.1.1
  });

  it("refuses NAT64-embedded private addresses", () => {
    expect(isSpecialAddress("64:ff9b::7f00:1")).toBe(true); // 64:ff9b::/96 → 127.0.0.1
  });

  it("allows genuine public addresses, including hex-mapped public v4", () => {
    expect(isSpecialAddress("1.1.1.1")).toBe(false);
    expect(isSpecialAddress("8.8.8.8")).toBe(false);
    expect(isSpecialAddress("::ffff:0808:0808")).toBe(false); // 8.8.8.8 mapped
    expect(isSpecialAddress("2606:4700:4700::1111")).toBe(false);
  });
});
