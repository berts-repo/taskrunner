import { createHash, randomBytes } from "node:crypto";
import * as fs from "node:fs";
import { dirname, join } from "node:path";

// Content-addressed artifact store under <state root>/artifacts/. Files are
// immutable once stored; metadata and links live in
// the event log and index, not here.

export interface StoredArtifact {
  sha256: string;
  size_bytes: number;
  locator: string;
}

export class ArtifactStore {
  constructor(readonly root: string) {}

  store(content: string | Uint8Array): StoredArtifact {
    const buf =
      typeof content === "string" ? Buffer.from(content, "utf8") : Buffer.from(content);
    const sha256 = createHash("sha256").update(buf).digest("hex");
    const locator = `${sha256.slice(0, 2)}/${sha256}`;
    const path = join(this.root, locator);
    if (!fs.existsSync(path)) {
      fs.mkdirSync(dirname(path), { recursive: true });
      // Write-then-rename so a crash never leaves a partial blob at the
      // content-addressed path.
      const tmp = join(dirname(path), `.tmp-${randomBytes(8).toString("hex")}`);
      fs.writeFileSync(tmp, buf);
      fs.renameSync(tmp, path);
    }
    return { sha256, size_bytes: buf.length, locator };
  }

  read(locator: string): Buffer {
    return fs.readFileSync(this.path(locator));
  }

  path(locator: string): string {
    return join(this.root, locator);
  }
}
