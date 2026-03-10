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
      `${traceSummary ? `${traceSummary}\n` : ""}${errorMsg}\n(Each execute() starts fresh. Include all needed tool() calls in your script.)`,
    );
  }
  return output;
}

export function buildTraceSummary(trace: TraceEntry[]): string | null {
  if (trace.length === 0) {
    return null;
  }

  const successful = trace.filter((e) => !e.error && e.output !== undefined);
  const failed = trace.filter((e) => e.error);

  const parts: string[] = [];

  if (successful.length > 0) {
    parts.push(
      "Tools completed successfully — their results are available as shown:",
    );
    for (const entry of successful) {
      const json = summarize(entry.output);
      const keys = extractKeys(entry.output);
      const keyHint = keys ? ` (keys: ${keys})` : "";
      parts.push(`  ${entry.toolName} = ${json}${keyHint}`);
    }
  }

  if (failed.length > 0) {
    for (const entry of failed) {
      parts.push(`${entry.toolName} FAILED: ${entry.error}`);
    }
  }

  return parts.join("\n");
}

/**
 * Build a concise summary of cached state for injection into LLM context.
 * Returns `null` if the cache is empty.
 */
export function buildStateSummary(
  stateCache: Record<string, unknown>,
): string | null {
  const entries = Object.entries(stateCache).filter(
    ([key]) => key.startsWith("last_") && key !== "last_result",
  );
  if (entries.length === 0) return null;

  const lines: string[] = ["[Prior state — available as pre-set variables]"];
  for (const [key, value] of entries) {
    const toolName = key.slice(5); // strip "last_"
    const json = summarize(value);
    const keys = extractKeys(value);
    const keyHint = keys ? ` (keys: ${keys})` : "";
    lines.push(`  ${key} = ${json}${keyHint}  // from ${toolName}()`);
  }

  if (stateCache["last_result"] !== undefined) {
    lines.push(`  last_result = ${summarize(stateCache["last_result"])}`);
  }

  return lines.join("\n");
}

function extractKeys(value: unknown): string | null {
  if (value != null && typeof value === "object" && !Array.isArray(value)) {
    const keys = Object.keys(value as Record<string, unknown>);
    if (keys.length > 0) {
      return keys.join(", ");
    }
  }
  return null;
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
