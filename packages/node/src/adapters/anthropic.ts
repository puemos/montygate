import type { Montygate } from "../engine.js";
import { unwrapExecutionResult } from "./utils.js";

/** Anthropic tool format (for use with the Anthropic SDK). */
export interface AnthropicTool {
  name: string;
  description: string;
  input_schema: {
    type: "object";
    properties: Record<string, unknown>;
    required?: string[];
  };
}

/**
 * Convert a Montygate engine into Anthropic-compatible tool definitions.
 * Returns [execute, search] tools.
 */
export function toAnthropic(engine: Montygate): AnthropicTool[] {
  const execSchema = engine.getExecuteToolInputSchema();
  const searchSchema = engine.getSearchToolInputSchema();

  return [
    {
      name: "execute",
      description: engine.getExecuteToolDescription(),
      input_schema: execSchema as AnthropicTool["input_schema"],
    },
    {
      name: "search",
      description: engine.getSearchToolDescription(),
      input_schema: searchSchema as AnthropicTool["input_schema"],
    },
  ];
}

/**
 * Handle an Anthropic tool call by dispatching to the engine.
 */
export async function handleAnthropicToolCall(
  engine: Montygate,
  name: string,
  input: Record<string, unknown>,
): Promise<unknown> {
  if (name === "execute") {
    const result = await engine.execute(
      input.code as string,
      input.inputs as Record<string, unknown> | undefined,
    );
    return unwrapExecutionResult(result);
  } else if (name === "search") {
    return engine.search(
      input.query as string,
      input.top_k as number | undefined,
    );
  }
  throw new Error(`Unknown tool: ${name}`);
}
