import { describe, expect, it } from "vitest";
import { resolveTier } from "../../src/domain/policy.js";

describe("resolveTier", () => {
  it("no extra domains is workspace-write", () => {
    expect(resolveTier([])).toBe("workspace-write");
  });

  it("extra domains make it networked", () => {
    expect(resolveTier(["registry.npmjs.org"])).toBe("networked");
  });
});
