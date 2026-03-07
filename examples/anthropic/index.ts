/**
 * Example: Using Montygate with the Anthropic SDK.
 *
 * This shows how to wrap existing Anthropic tool definitions directly —
 * no need to rewrite schemas. Montygate auto-detects the format.
 */
import { Montygate } from "montygate";
// import Anthropic from "@anthropic-ai/sdk";

// Your existing Anthropic tool definitions — pass them straight to Montygate
const gate = new Montygate({
  tools: [
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
  ],
  handlers: {
    lookup_order: async (args: unknown) => {
      const { order_id } = args as { order_id: string };
      return { id: order_id, status: "shipped", email: "customer@example.com" };
    },
    create_ticket: async (args: unknown) => {
      const { subject, body } = args as { subject: string; body: string };
      return { ticket_id: `TKT-${Date.now()}`, subject, body };
    },
  },
});

// Get Anthropic-compatible tool definitions for the LLM
const tools = gate.anthropic();
console.log("Anthropic tools:", JSON.stringify(tools, null, 2));

// In a real app:
//
// const client = new Anthropic();
// const response = await client.messages.create({
//   model: "claude-sonnet-4-20250514",
//   max_tokens: 1024,
//   tools: gate.anthropic(),
//   messages: [{ role: "user", content: "Look up order ORD-123 and create a ticket" }],
// });
//
// for (const block of response.content) {
//   if (block.type === "tool_use") {
//     const result = await gate.handleToolCall(block.name, block.input);
//     // Send result back to Claude...
//   }
// }

// Simulate
async function main() {
  const result = await gate.handleToolCall("execute", {
    code: `
order = tool('lookup_order', order_id='ORD-123')
ticket = tool('create_ticket', subject='Issue with ' + order['id'], body='Status: ' + order['status'])
ticket
    `,
  });

  console.log("\nTool call result:", JSON.stringify(result, null, 2));
}

main().catch(console.error);
