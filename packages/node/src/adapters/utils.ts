import type { ExecutionResult, TraceEntry } from "../types.js";

/**
 * Unwrap an execution result, throwing if the sandbox returned an error.
 *
 * Sandbox errors (e.g. NameError, SyntaxError) come back as
 * `{ status: "error", error: "..." }` in result.output — which looks like
 * normal output to the LLM. This function detects that pattern and throws
 * so that the agent loop's catch block can set `is_error: true`.
 */
export function unwrapExecutionResult(result: ExecutionResult): unknown {
  const output = result.output;
  if (
    output != null &&
    typeof output === "object" &&
    !Array.isArray(output) &&
    (output as Record<string, unknown>).status === "error"
  ) {
    const errorMsg =
      (output as Record<string, unknown>).error ?? "unknown sandbox error";
    const traceSummary = buildTraceSummary(result.trace);
    throw new Error(
      `${traceSummary ? `${traceSummary}\n` : ""}${errorMsg} (Note: each execute() runs in a fresh sandbox — variables from previous calls are not available.)`,
    );
  }
  return output;
}

export function buildTraceSummary(trace: TraceEntry[]): string | null {
  if (trace.length === 0) {
    return null;
  }

  const parts = trace.map((entry) => {
    if (entry.error) {
      return `${entry.toolName} FAILED: ${entry.error}`;
    }
    if (entry.output !== undefined) {
      return `${entry.toolName} OK -> ${summarize(entry.output)}`;
    }
    return `${entry.toolName} OK`;
  });

  return `Previous tool calls: ${parts.join(" | ")}`;
}

function summarize(value: unknown): string {
  try {
    const json = JSON.stringify(value);
    if (json == null) {
      return String(value);
    }
    return json.length > 200 ? `${json.slice(0, 197)}...` : json;
  } catch {
    return String(value);
  }
}
