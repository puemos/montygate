/**
 * Example: Using Montygate with the Anthropic SDK.
 *
 * This shows how to wrap existing Anthropic tool definitions directly —
 * no need to rewrite schemas. Montygate auto-detects the format.
 */
import { Montygate, toAnthropic, handleAnthropicToolCall } from "montygate";
// import Anthropic from "@anthropic-ai/sdk";

// Your existing Anthropic tool definitions — pass them straight to Montygate
const myAnthropicTools = [
  {
    name: "lookup_order",
    description: "Look up order details by order ID",
    input_schema: {
      type: "object",
      properties: { order_id: { type: "string" } },
      required: ["order_id"],
    },
  },
  {
    name: "create_ticket",
    description: "Create a support ticket",
    input_schema: {
      type: "object",
      properties: {
        subject: { type: "string" },
        body: { type: "string" },
      },
      required: ["subject", "body"],
    },
  },
];

// Just wrap your existing tools + provide handlers
const engine = new Montygate();
engine.tools(myAnthropicTools, {
  lookup_order: async (args: unknown) => {
    const { order_id } = args as { order_id: string };
    return { id: order_id, status: "shipped", email: "customer@example.com" };
  },
  create_ticket: async (args: unknown) => {
    const { subject, body } = args as { subject: string; body: string };
    return { ticket_id: `TKT-${Date.now()}`, subject, body };
  },
});

// Alternative: register tools one-by-one with Zod schemas (still supported)
// import { z } from "zod";
// engine.tool("lookup_order", {
//   description: "Look up order details by order ID",
//   params: z.object({ order_id: z.string() }),
//   run: async ({ order_id }) => ({ id: order_id, status: "shipped", email: "customer@example.com" }),
// });

// Get Anthropic-compatible tool definitions
const tools = toAnthropic(engine);
console.log("Anthropic tools:", JSON.stringify(tools, null, 2));

// In a real app, you'd pass these to the Anthropic SDK:
//
// const client = new Anthropic();
// const response = await client.messages.create({
//   model: "claude-sonnet-4-20250514",
//   max_tokens: 1024,
//   tools,
//   messages: [{ role: "user", content: "Look up order ORD-123 and create a ticket" }],
// });
//
// // Handle tool use blocks
// for (const block of response.content) {
//   if (block.type === "tool_use") {
//     const result = await handleAnthropicToolCall(engine, block.name, block.input);
//     // Send result back to Claude...
//   }
// }

// Simulate handling a tool call
async function main() {
  const result = await handleAnthropicToolCall(engine, "execute", {
    code: `
order = tool('lookup_order', order_id='ORD-123')
ticket = tool('create_ticket', subject='Issue with ' + order['id'], body='Status: ' + order['status'])
ticket
    `,
  });

  console.log("\nTool call result:", JSON.stringify(result, null, 2));
}

main().catch(console.error);
