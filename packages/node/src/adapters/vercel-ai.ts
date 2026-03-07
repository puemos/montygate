import { z } from "zod";
import type { Montygate } from "../engine.js";
import { unwrapExecutionResult } from "./utils.js";

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
 *
 * Note: Vercel AI requires Zod schemas (not JSON Schema), so the parameter
 * shapes here must mirror the canonical schemas from the Rust core
 * (`engine.getExecuteToolInputSchema()` / `engine.getSearchToolInputSchema()`).
 */
export function toVercelAI(engine: Montygate): Record<string, VercelAIToolDef> {
  return {
    execute: {
      description: engine.getExecuteToolDescription(),
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
        return unwrapExecutionResult(result);
      },
    },
    search: {
      description: engine.getSearchToolDescription(),
      parameters: z.object({
        query: z.string().describe("Search query"),
        top_k: z.number().optional().describe("Maximum number of results"),
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
