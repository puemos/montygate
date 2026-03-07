#!/usr/bin/env npx tsx
/**
 * Manual test: real Claude agent loop with Montygate.
 *
 * Verifies that the execute/search tool definitions work end-to-end
 * with a real LLM — Claude writes Python scripts, calls tools, gets results.
 *
 * Run from packages/node/:
 *   ANTHROPIC_API_KEY=sk-... npx tsx src/manual-agent-loop.ts
 */

import Anthropic from "@anthropic-ai/sdk";
import { z } from "zod";
import { handleAnthropicToolCall, Montygate, toAnthropic } from "./index.js";

// ── Setup ──────────────────────────────────────────────────────────────────

const client = new Anthropic();

const gate = new Montygate({
  limits: { maxConcurrent: 5, timeoutMs: 30_000 },
});

gate.tool("lookup_order", {
  description:
    "Look up order details by order ID. Returns order status, customer email, items, and tracking info.",
  params: z.object({
    order_id: z.string().describe("The order ID, e.g. ORD-123"),
  }),
  returns: z.object({
    id: z.string(),
    status: z.string(),
    email: z.string(),
    items: z.array(z.string()),
    tracking: z.string(),
  }),
  run: async ({ order_id }) => {
    console.log(`  [tool] lookup_order(${order_id})`);
    return {
      id: order_id,
      status: "shipped",
      email: "alice@example.com",
      items: ["Widget A", "Gadget B"],
      tracking: "TR-98765",
    };
  },
});

gate.tool("create_ticket", {
  description: "Create a customer support ticket. Returns the new ticket ID.",
  params: z.object({
    subject: z.string().describe("Ticket subject line"),
    body: z.string().describe("Ticket body / description"),
    priority: z.enum(["low", "medium", "high"]).describe("Ticket priority"),
  }),
  returns: z.object({
    ticket_id: z.string(),
    subject: z.string(),
    body: z.string(),
    priority: z.string(),
    status: z.string(),
  }),
  run: async ({ subject, body, priority }) => {
    console.log(
      `  [tool] create_ticket(subject="${subject}", priority=${priority})`,
    );
    return {
      ticket_id: `TKT-${Date.now()}`,
      subject,
      body,
      priority,
      status: "open",
    };
  },
});

gate.tool("send_email", {
  description: "Send an email to a recipient.",
  params: z.object({
    to: z.string().describe("Recipient email address"),
    subject: z.string().describe("Email subject"),
    body: z.string().describe("Email body text"),
  }),
  run: async ({ to, subject }) => {
    console.log(`  [tool] send_email(to="${to}", subject="${subject}")`);
    return { sent: true, to, subject };
  },
});

gate.tool("get_weather", {
  description: "Get current weather for a city.",
  params: z.object({
    city: z.string().describe("City name"),
  }),
  run: async ({ city }) => {
    console.log(`  [tool] get_weather(${city})`);
    return { city, temp_c: 22, condition: "partly cloudy", humidity: 65 };
  },
});

// ── Agent loop ─────────────────────────────────────────────────────────────

const tools = toAnthropic(gate) as Anthropic.Messages.Tool[];

async function agentLoop(userMessage: string) {
  console.log(`\n${"=".repeat(70)}`);
  console.log(`User: ${userMessage}`);
  console.log("=".repeat(70));

  const messages: Anthropic.Messages.MessageParam[] = [
    { role: "user", content: userMessage },
  ];

  let turns = 0;
  const maxTurns = 10;

  while (turns < maxTurns) {
    turns++;
    console.log(`\n--- Turn ${turns} ---`);

    const response = await client.messages.create({
      model: "claude-haiku-4-5-20251001",
      max_tokens: 2048,
      system: gate.getSystemPrompt(),
      tools,
      messages,
    });

    const textBlocks: string[] = [];
    const toolUseBlocks: Anthropic.Messages.ToolUseBlock[] = [];

    for (const block of response.content) {
      if (block.type === "text") {
        textBlocks.push(block.text);
      } else if (block.type === "tool_use") {
        toolUseBlocks.push(block);
      }
    }

    if (textBlocks.length > 0) {
      console.log(`\nClaude: ${textBlocks.join("\n")}`);
    }

    if (response.stop_reason === "end_turn" && toolUseBlocks.length === 0) {
      console.log("\n[agent loop complete]");
      break;
    }

    if (toolUseBlocks.length > 0) {
      messages.push({ role: "assistant", content: response.content });

      const toolResults: Anthropic.Messages.ToolResultBlockParam[] = [];

      for (const block of toolUseBlocks) {
        console.log(`\nTool call: ${block.name}`);
        console.log(`  Input: ${JSON.stringify(block.input, null, 2)}`);

        try {
          const result = await handleAnthropicToolCall(
            gate,
            block.name,
            block.input as Record<string, unknown>,
          );
          console.log(`  Result: ${JSON.stringify(result, null, 2)}`);

          toolResults.push({
            type: "tool_result",
            tool_use_id: block.id,
            content: JSON.stringify(result),
          });
        } catch (err) {
          const errorMsg = err instanceof Error ? err.message : String(err);
          console.log(`  Error: ${errorMsg}`);

          toolResults.push({
            type: "tool_result",
            tool_use_id: block.id,
            content: errorMsg,
            is_error: true,
          });
        }
      }

      messages.push({ role: "user", content: toolResults });
    }
  }

  if (turns >= maxTurns) {
    console.log("\n[max turns reached]");
  }

  // Print traces
  const traces = gate.getTraces();
  if (traces.length > 0) {
    console.log(`\n--- Execution traces (${traces.length} tool calls) ---`);
    for (const t of traces) {
      const status = t.error ? `ERROR: ${t.error}` : "OK";
      console.log(`  ${t.toolName}: ${t.durationMs}ms [${status}]`);
    }
  }
  gate.clearTraces();
}

// ── Run scenarios ──────────────────────────────────────────────────────────

async function main() {
  console.log("Montygate agent loop — manual test");
  console.log(`Registered tools: ${gate.toolCount}`);
  console.log(`\nTool catalog:\n${gate.getToolCatalog()}`);

  // Scenario 1: Multi-tool orchestration
  // Claude should write a script that calls multiple tools and returns a combined result
  await agentLoop(
    "Look up order ORD-456, create a high priority support ticket about it being late, " +
      "then email the customer with the ticket ID. Return the ticket details.",
  );

  // Scenario 2: Search then execute
  // Claude should use the search tool first, then write a script
  await agentLoop(
    "What tools do you have related to weather? Use them to check the weather in Tokyo.",
  );

  // Scenario 3: Script with conditional logic
  // Claude should write a script with branching
  await agentLoop(
    "Look up order ORD-789. If the status is 'shipped', send a shipping confirmation email " +
      "to the customer. If not, create a support ticket. Return what action was taken.",
  );
}

main().catch(console.error);
