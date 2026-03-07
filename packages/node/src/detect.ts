export type ToolFormat =
  | "openai-chat"
  | "openai-responses"
  | "anthropic-raw"
  | "anthropic-zod"
  | "openai-agents"
  | "vercel-ai"
  | "unknown";

/**
 * Check if a value looks like a Zod schema (has _def with typeName string).
 */
export function isZodSchema(value: unknown): boolean {
  if (value == null || typeof value !== "object") return false;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const def = (value as any)._def;
  return (
    def != null && typeof def === "object" && typeof def.typeName === "string"
  );
}

/**
 * Auto-detect the format of a tool definition.
 * Detection order: most specific first to resolve ambiguities.
 */
export function detectFormat(tool: unknown): ToolFormat {
  if (tool == null || typeof tool !== "object") return "unknown";

  const t = tool as Record<string, unknown>;

  // OpenAI Chat Completions: { type: "function", function: { name } }
  if (
    t.type === "function" &&
    t.function != null &&
    typeof t.function === "object" &&
    typeof (t.function as Record<string, unknown>).name === "string"
  ) {
    return "openai-chat";
  }

  // Anthropic betaZodTool: { name, inputSchema (Zod), run }
  // Must check before anthropic-raw since both have .name
  if (
    typeof t.name === "string" &&
    isZodSchema(t.inputSchema) &&
    typeof t.run === "function"
  ) {
    return "anthropic-zod";
  }

  // OpenAI Agents SDK: { name, parameters, execute }
  // Must check before openai-responses since both can have .name
  if (
    typeof t.name === "string" &&
    t.parameters != null &&
    typeof t.execute === "function"
  ) {
    return "openai-agents";
  }

  // OpenAI Responses API: { type: "function", name } (flat)
  if (t.type === "function" && typeof t.name === "string") {
    return "openai-responses";
  }

  // Anthropic raw: { name, input_schema }
  if (typeof t.name === "string" && t.input_schema != null) {
    return "anthropic-raw";
  }

  // Vercel AI SDK: { description, execute } — no .name (name comes from object key)
  if (typeof t.description === "string" && typeof t.execute === "function") {
    return "vercel-ai";
  }

  return "unknown";
}
