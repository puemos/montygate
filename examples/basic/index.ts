import { Montygate } from "montygate";
import { z } from "zod";

// 1. Create engine
const engine = new Montygate({
  retry: { maxRetries: 3, baseDelayMs: 100 },
  limits: { timeoutMs: 30_000, maxConcurrent: 5 },
});

// 2. Register tools
engine.tool("lookup_order", {
  description: "Look up order details by order ID",
  params: z.object({ order_id: z.string() }),
  run: async ({ order_id }) => ({
    id: order_id,
    status: "shipped",
    email: "customer@example.com",
    items: ["Widget A", "Widget B"],
  }),
});

engine.tool("create_ticket", {
  description: "Create a support ticket",
  params: z.object({ subject: z.string(), body: z.string() }),
  run: async ({ subject, body }) => ({
    ticket_id: `TKT-${Date.now()}`,
    subject,
    body,
    status: "open",
  }),
});

engine.tool("send_email", {
  description: "Send an email notification",
  params: z.object({ to: z.string(), subject: z.string(), body: z.string() }),
  run: async ({ to, subject }) => ({
    sent: true,
    to,
    subject,
  }),
});

// 3. Execute a multi-tool script — only the final result returns
async function main() {
  console.log("Registered tools:", engine.toolCount);
  console.log("Tool catalog:\n" + engine.getToolCatalog());

  // WITHOUT Montygate: 3 LLM round trips, all intermediate results as tokens
  // WITH Montygate: 1 script, 1 result
  const result = await engine.execute(`
order = tool('lookup_order', order_id='ORD-123')
ticket = tool('create_ticket',
  subject='Late order ' + order['id'],
  body='Customer ' + order['email'] + ' has a late order with items: ' + str(order['items'])
)
tool('send_email',
  to=order['email'],
  subject=ticket['subject'],
  body='Your ticket ' + ticket['ticket_id'] + ' has been created.'
)
ticket
  `);

  console.log("\n--- Result (only this goes back to the LLM) ---");
  console.log(JSON.stringify(result.output, null, 2));

  console.log("\n--- Execution stats ---");
  console.log(`Total duration: ${result.stats.totalDurationMs}ms`);
  console.log(`Tool calls: ${result.stats.externalCalls}`);

  console.log("\n--- Trace ---");
  for (const entry of result.trace) {
    console.log(`  ${entry.toolName}: ${entry.durationMs}ms`);
  }

  // Search for tools
  console.log("\n--- Search: 'email' ---");
  const searchResults = engine.search("email");
  for (const r of searchResults) {
    console.log(`  ${r.name}: ${r.description}`);
  }
}

main().catch(console.error);
