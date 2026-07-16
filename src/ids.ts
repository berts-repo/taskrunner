import { ulid } from "ulid";

// Approved record ID shape: prefixed lowercase ULIDs (docs/specs/NAMING.md).
export type IdPrefix =
  | "proj"
  | "sess"
  | "task"
  | "turn"
  | "wsess"
  | "evt"
  | "art"
  | "appr";

export function newId(prefix: IdPrefix): string {
  return `${prefix}_${ulid().toLowerCase()}`;
}
