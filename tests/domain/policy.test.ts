import { describe, expect, it } from "vitest";
import { resolveTier } from "../../src/domain/policy.js";

describe("resolveTier", () => {
  it("docker with no extra domains is workspace-write", () => {
    expect(resolveTier("docker", [])).toBe("workspace-write");
  });

  it("extra domains make it networked", () => {
    expect(resolveTier("docker", ["registry.npmjs.org"])).toBe("networked");
  });

  it("host runtime is always privileged, even without domains", () => {
    expect(resolveTier("host", [])).toBe("privileged");
    expect(resolveTier("host", ["registry.npmjs.org"])).toBe("privileged");
  });
});
