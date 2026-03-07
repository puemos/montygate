import type { Montygate } from "../engine.js";

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
  const catalog = engine.getToolCatalog();

  return [
    {
      type: "function",
      function: {
        name: "execute",
        description: `Execute a Python script with access to these tools:\n${catalog}\nUse tool('name', key=value) to call tools. The last expression is returned.`,
        parameters: {
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
    },
    {
      type: "function",
      function: {
        name: "search",
        description: "Search for available tools by keyword",
        parameters: {
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
    return JSON.stringify(result.output);
  } else if (name === "search") {
    const results = engine.search(
      input.query as string,
      input.top_k as number | undefined,
    );
    return JSON.stringify(results);
  }
  throw new Error(`Unknown tool: ${name}`);
}
