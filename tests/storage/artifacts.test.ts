import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import { ArtifactStore } from "../../src/storage/artifacts.js";
import { tempDir } from "../helpers.js";

describe("ArtifactStore", () => {
  it("stores and reads back content by locator", () => {
    const store = new ArtifactStore(tempDir("artifacts"));
    const stored = store.store("hello artifacts");

    expect(stored.sha256).toBe(createHash("sha256").update("hello artifacts").digest("hex"));
    expect(stored.size_bytes).toBe(Buffer.byteLength("hello artifacts"));
    expect(stored.locator).toBe(`${stored.sha256.slice(0, 2)}/${stored.sha256}`);
    expect(store.read(stored.locator).toString("utf8")).toBe("hello artifacts");
  });

  it("deduplicates identical content", () => {
    const store = new ArtifactStore(tempDir("artifacts"));
    const a = store.store("same bytes");
    const b = store.store(Buffer.from("same bytes"));
    expect(b).toEqual(a);
  });
});
