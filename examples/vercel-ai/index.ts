/**
 * Example: Using Montygate with the Vercel AI SDK.
 *
 * This shows how to wrap existing Vercel AI-style tools directly —
 * no need to rewrite schemas. Montygate auto-detects the format.
 */
import { Montygate, toVercelAI } from "montygate";
import { z } from "zod";
// import { generateText } from "ai";
// import { anthropic } from "@ai-sdk/anthropic";

// Your existing Vercel AI-style tools — pass them straight to Montygate
const myVercelTools = {
  search_docs: {
    description: "Search documentation by query",
    parameters: z.object({ query: z.string(), limit: z.number().optional() }),
    execute: async (args: unknown) => {
      const { query, limit } = args as { query: string; limit?: number };
      return {
        results: [
          { title: `Result for "${query}" #1`, url: "https://docs.example.com/1" },
          { title: `Result for "${query}" #2`, url: "https://docs.example.com/2" },
        ].slice(0, limit ?? 2),
      };
    },
  },
  create_summary: {
    description: "Create a summary of given text",
    parameters: z.object({ text: z.string(), maxWords: z.number().optional() }),
    execute: async (args: unknown) => {
      const { text, maxWords } = args as { text: string; maxWords?: number };
      return {
        summary: text.slice(0, (maxWords ?? 50) * 5) + "...",
        wordCount: maxWords ?? 50,
      };
    },
  },
};

// Just wrap your existing tools — handlers are already embedded
const engine = new Montygate();
engine.tools(myVercelTools);

// Alternative: register tools one-by-one with Zod schemas (still supported)
// engine.tool("search_docs", {
//   description: "Search documentation by query",
//   params: z.object({ query: z.string(), limit: z.number().optional() }),
//   run: async ({ query, limit }) => ({ results: [...] }),
// });

// Get Vercel AI SDK-compatible tools
const tools = toVercelAI(engine);
console.log("Tool names:", Object.keys(tools));
console.log("Execute description:", tools.execute.description.slice(0, 100) + "...");

// In a real app:
//
// const { text, toolResults } = await generateText({
//   model: anthropic("claude-sonnet-4-20250514"),
//   tools,
//   prompt: "Search for authentication docs and summarize the results",
// });

// Simulate
async function main() {
  const result = await tools.execute.execute({
    code: `
docs = tool('search_docs', query='authentication', limit=2)
titles = [d['title'] for d in docs['results']]
summary = tool('create_summary', text=str(titles), maxWords=20)
summary
    `,
  });

  console.log("\nResult:", JSON.stringify(result, null, 2));
}

main().catch(console.error);
