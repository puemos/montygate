import { z } from "zod";
import type { Montygate } from "../engine.js";

/**
 * Vercel AI SDK tool definition shape.
 * Compatible with the `tool()` helper from `ai` package.
 */
export interface VercelAIToolDef {
  description: string;
  parameters: z.ZodType;
  execute: (args: Record<string, unknown>) => Promise<unknown>;
}

/**
 * Convert a Montygate engine into Vercel AI SDK-compatible tool definitions.
 * Returns { execute, search } object for use with `generateText({ tools: ... })`.
 */
export function toVercelAI(engine: Montygate): Record<string, VercelAIToolDef> {
  const catalog = engine.getToolCatalog();

  return {
    execute: {
      description: `Execute a Python script with access to these tools:\n${catalog}\nUse tool('name', key=value) to call tools. The last expression is returned.`,
      parameters: z.object({
        code: z.string().describe("Python script to execute"),
        inputs: z
          .record(z.unknown())
          .optional()
          .describe("Variables to inject into the script"),
      }),
      execute: async (args) => {
        const result = await engine.execute(
          args.code as string,
          args.inputs as Record<string, unknown> | undefined,
        );
        return result.output;
      },
    },
    search: {
      description: "Search for available tools by keyword",
      parameters: z.object({
        query: z.string().describe("Search query"),
        top_k: z
          .number()
          .optional()
          .describe("Maximum number of results"),
      }),
      execute: async (args) => {
        return engine.search(
          args.query as string,
          args.top_k as number | undefined,
        );
      },
    },
  };
}
