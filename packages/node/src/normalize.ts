import { isZodSchema, type ToolFormat } from "./detect.js";
import { zodToJsonSchema } from "./schema.js";

export interface NormalizedTool {
  name: string;
  description?: string;
  inputSchema: Record<string, unknown>;
  handler?: (args: unknown) => Promise<unknown>;
}

export type ToolHandlerMap = Record<
  string,
  (args: unknown) => Promise<unknown>
>;

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type AnyToolDefinition = any;

/**
 * Normalize a single tool definition into a standard shape.
 * @param tool - The raw tool definition in any supported format.
 * @param format - Pre-detected format (or auto-detect if omitted).
 * @param handlers - Optional handler map for formats without embedded handlers.
 * @param keyName - Object key name (used as tool name for vercel-ai format).
 */
export function normalizeTool(
  tool: AnyToolDefinition,
  format: ToolFormat,
  handlers?: ToolHandlerMap,
  keyName?: string,
): NormalizedTool {
  switch (format) {
    case "openai-chat":
      return normalizeOpenAIChat(tool, handlers);
    case "openai-responses":
      return normalizeOpenAIResponses(tool, handlers);
    case "anthropic-raw":
      return normalizeAnthropicRaw(tool, handlers);
    case "anthropic-zod":
      return normalizeAnthropicZod(tool);
    case "openai-agents":
      return normalizeOpenAIAgents(tool);
    case "vercel-ai":
      return normalizeVercelAI(tool, keyName as string);
    default:
      throw new Error(`Unknown tool format: cannot normalize tool`);
  }
}

function normalizeOpenAIChat(
  tool: AnyToolDefinition,
  handlers?: ToolHandlerMap,
): NormalizedTool {
  const fn = tool.function;
  return {
    name: fn.name,
    description: fn.description,
    inputSchema: fn.parameters ?? { type: "object", properties: {} },
    handler: handlers?.[fn.name],
  };
}

function normalizeOpenAIResponses(
  tool: AnyToolDefinition,
  handlers?: ToolHandlerMap,
): NormalizedTool {
  return {
    name: tool.name,
    description: tool.description,
    inputSchema: tool.parameters ?? { type: "object", properties: {} },
    handler: handlers?.[tool.name],
  };
}

function normalizeAnthropicRaw(
  tool: AnyToolDefinition,
  handlers?: ToolHandlerMap,
): NormalizedTool {
  return {
    name: tool.name,
    description: tool.description,
    inputSchema: tool.input_schema ?? { type: "object", properties: {} },
    handler: handlers?.[tool.name],
  };
}

function normalizeAnthropicZod(tool: AnyToolDefinition): NormalizedTool {
  const schema = isZodSchema(tool.inputSchema)
    ? zodToJsonSchema(tool.inputSchema)
    : (tool.inputSchema as Record<string, unknown>);

  return {
    name: tool.name,
    description: tool.description,
    inputSchema: schema,
    handler: tool.run,
  };
}

function normalizeOpenAIAgents(tool: AnyToolDefinition): NormalizedTool {
  const schema = isZodSchema(tool.parameters)
    ? zodToJsonSchema(tool.parameters)
    : (tool.parameters as Record<string, unknown>);

  return {
    name: tool.name,
    description: tool.description,
    inputSchema: schema,
    handler: tool.execute,
  };
}

function normalizeVercelAI(
  tool: AnyToolDefinition,
  keyName: string,
): NormalizedTool {
  const rawSchema = tool.parameters ?? tool.inputSchema;
  const schema = isZodSchema(rawSchema)
    ? zodToJsonSchema(rawSchema)
    : ((rawSchema as Record<string, unknown>) ?? {
        type: "object",
        properties: {},
      });

  return {
    name: keyName,
    description: tool.description,
    inputSchema: schema,
    handler: tool.execute,
  };
}
