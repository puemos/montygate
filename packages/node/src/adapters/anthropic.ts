import type { Montygate } from "../engine.js";

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
  const catalog = engine.getToolCatalog();

  return [
    {
      name: "execute",
      description: `Execute a Python script with access to these tools:\n${catalog}\nUse tool('name', key=value) to call tools. The last expression is returned.`,
      input_schema: {
        type: "object",
        properties: {
          code: {
            type: "string",
            description: "Python script to execute",
          },
          inputs: {
            type: "object",
            description: "Variables to inject into the script",
          },
        },
        required: ["code"],
      },
    },
    {
      name: "search",
      description: "Search for available tools by keyword",
      input_schema: {
        type: "object",
        properties: {
          query: {
            type: "string",
            description: "Search query",
          },
          top_k: {
            type: "number",
            description: "Maximum number of results",
          },
        },
        required: ["query"],
      },
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
    return result.output;
  } else if (name === "search") {
    return engine.search(
      input.query as string,
      input.top_k as number | undefined,
    );
  }
  throw new Error(`Unknown tool: ${name}`);
}
