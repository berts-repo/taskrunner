// Error codes surfaced by the MCP tools.
export type ErrorCode =
  | "invalid_request"
  | "not_found"
  | "not_configured"
  | "approval_required"
  | "policy_denied"
  | "capture_unavailable"
  | "worker_unavailable"
  | "worker_failed"
  | "conflict"
  | "internal_error";

export class ToolError extends Error {
  constructor(
    readonly code: ErrorCode,
    message: string,
  ) {
    super(message);
  }
}
