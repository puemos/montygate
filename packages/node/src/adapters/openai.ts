import type { Montygate } from "../engine.js";
import { unwrapExecutionResult } from "./utils.js";

/** OpenAI function tool format. */
export interface OpenAITool {
  type: "function";
  function: {
    name: string;
    description: string;
    parameters: {
      type: "object";
      properties: Record<string, unknown>;
      required?: string[];
    };
  };
}

/**
 * Convert a Montygate engine into OpenAI-compatible tool definitions.
 * Returns [execute, search] function tools.
 */
export function toOpenAI(engine: Montygate): OpenAITool[] {
  const execSchema = engine.getExecuteToolInputSchema();
  const searchSchema = engine.getSearchToolInputSchema();

  return [
    {
      type: "function",
      function: {
        name: "execute",
        description: engine.getExecuteToolDescription(),
        parameters: execSchema as OpenAITool["function"]["parameters"],
      },
    },
    {
      type: "function",
      function: {
        name: "search",
        description: engine.getSearchToolDescription(),
        parameters: searchSchema as OpenAITool["function"]["parameters"],
      },
    },
  ];
}

/**
 * Handle an OpenAI tool call by dispatching to the engine.
 */
export async function handleOpenAIToolCall(
  engine: Montygate,
  name: string,
  args: string,
): Promise<string> {
  const input = JSON.parse(args) as Record<string, unknown>;

  if (name === "execute") {
    const result = await engine.execute(
      input.code as string,
      input.inputs as Record<string, unknown> | undefined,
    );
    return JSON.stringify(unwrapExecutionResult(result));
  } else if (name === "search") {
    const results = engine.search(
      input.query as string,
      input.top_k as number | undefined,
    );
    return JSON.stringify(results);
  }
  throw new Error(`Unknown tool: ${name}`);
}
