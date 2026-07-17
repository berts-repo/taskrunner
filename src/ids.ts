import { ulid } from "ulid";

// Record IDs are prefixed lowercase ULIDs.
type IdPrefix =
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
