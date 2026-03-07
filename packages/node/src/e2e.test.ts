import { describe, it, expect, beforeEach } from "vitest";
import { z } from "zod";
import { Montygate } from "./engine.js";
import { toAnthropic, handleAnthropicToolCall } from "./adapters/anthropic.js";
import { toOpenAI, handleOpenAIToolCall } from "./adapters/openai.js";
import { toVercelAI } from "./adapters/vercel-ai.js";

/**
 * End-to-end tests exercising the full vertical slice:
 * TypeScript SDK → NAPI bindings → Rust engine (Monty sandbox) → JS callbacks → result
 *
 * These tests use the real Montygate class (not NativeEngine directly),
 * real Zod schemas, and real async JS tool callbacks that cross the
 * NAPI ThreadsafeFunction bridge.
 */

describe("e2e: multi-tool orchestration", () => {
  let gate: Montygate;

  beforeEach(() => {
    gate = new Montygate();
  });

  it("executes a 3-tool pipeline with data flowing between tools", async () => {
    gate.tool("lookup_order", {
      description: "Look up order by ID",
      params: z.object({ order_id: z.string() }),
      run: async ({ order_id }) => ({
        id: order_id,
        status: "shipped",
        email: "alice@example.com",
        tracking: "TR-789",
      }),
    });

    gate.tool("create_ticket", {
      description: "Create a support ticket",
      params: z.object({ subject: z.string(), customer_email: z.string() }),
      run: async ({ subject, customer_email }) => ({
        ticket_id: "TKT-100",
        subject,
        customer_email,
      }),
    });

    gate.tool("send_email", {
      description: "Send an email",
      params: z.object({ to: z.string(), body: z.string() }),
      run: async ({ to, body }) => ({ sent: true, to, body }),
    });

    const result = await gate.execute(`
order = tool('lookup_order', order_id='ORD-456')
ticket = tool('create_ticket', subject='Tracking for ' + order['id'], customer_email=order['email'])
notification = tool('send_email', to=order['email'], body='Ticket ' + ticket['ticket_id'] + ' created for ' + order['tracking'])
{"ticket": ticket, "notification": notification}
    `);

    // Only the final expression is returned
    const output = result.output as {
      ticket: { ticket_id: string; subject: string; customer_email: string };
      notification: { sent: boolean; to: string; body: string };
    };
    expect(output.ticket.ticket_id).toBe("TKT-100");
    expect(output.ticket.subject).toBe("Tracking for ORD-456");
    expect(output.ticket.customer_email).toBe("alice@example.com");
    expect(output.notification.sent).toBe(true);
    expect(output.notification.to).toBe("alice@example.com");
    expect(output.notification.body).toBe(
      "Ticket TKT-100 created for TR-789",
    );

    // All 3 tool calls traced
    expect(result.trace).toHaveLength(3);
    expect(result.trace[0].toolName).toBe("lookup_order");
    expect(result.trace[1].toolName).toBe("create_ticket");
    expect(result.trace[2].toolName).toBe("send_email");

    // Stats reflect 3 external calls
    expect(result.stats.externalCalls).toBe(3);
    expect(result.stats.totalDurationMs).toBeGreaterThan(0);
  });

  it("passes data between tools — downstream tool receives upstream output", async () => {
    const receivedArgs: unknown[] = [];

    gate.tool("fetch_user", {
      params: z.object({ user_id: z.string() }),
      run: async ({ user_id }) => ({ name: "Bob", role: "admin", user_id }),
    });

    gate.tool("check_permissions", {
      params: z.object({ role: z.string(), action: z.string() }),
      run: async (args) => {
        receivedArgs.push(args);
        return { allowed: args.role === "admin" };
      },
    });

    const result = await gate.execute(`
user = tool('fetch_user', user_id='U-42')
perm = tool('check_permissions', role=user['role'], action='delete')
perm['allowed']
    `);

    expect(result.output).toBe(true);
    // Verify the JS callback received the correct upstream data
    expect(receivedArgs[0]).toEqual({ role: "admin", action: "delete" });
  });
});

describe("e2e: input injection", () => {
  it("injects variables into the Monty sandbox", async () => {
    const gate = new Montygate();

    gate.tool("greet", {
      params: z.object({ name: z.string(), greeting: z.string() }),
      run: async ({ name, greeting }) => `${greeting}, ${name}!`,
    });

    const result = await gate.execute(
      `tool('greet', name=user_name, greeting=msg)`,
      { user_name: "Charlie", msg: "Hello" },
    );

    expect(result.output).toBe("Hello, Charlie!");
  });

  it("injects complex objects as inputs", async () => {
    const gate = new Montygate();

    gate.tool("process", {
      params: z.object({ items: z.array(z.number()) }),
      run: async ({ items }) => items.reduce((a: number, b: number) => a + b, 0),
    });

    const result = await gate.execute(`tool('process', items=data['values'])`, {
      data: { values: [1, 2, 3, 4, 5] },
    });

    expect(result.output).toBe(15);
  });
});

describe("e2e: error handling", () => {
  it("tool error is catchable in Python script", async () => {
    const gate = new Montygate();

    gate.tool("flaky", {
      params: z.object({}),
      run: async () => {
        throw new Error("database connection lost");
      },
    });

    gate.tool("fallback", {
      params: z.object({}),
      run: async () => ({ source: "cache", data: "stale-but-ok" }),
    });

    const result = await gate.execute(`
try:
    tool('flaky')
except:
    result = tool('fallback')
result
    `);

    const output = result.output as { source: string; data: string };
    expect(output.source).toBe("cache");
    expect(output.data).toBe("stale-but-ok");

    // Both calls should appear in trace
    expect(result.trace).toHaveLength(2);
    expect(result.trace[0].toolName).toBe("flaky");
    expect(result.trace[0].error).toBeDefined();
    expect(result.trace[1].toolName).toBe("fallback");
    expect(result.trace[1].output).toBeDefined();
  });

  it("script syntax error returns error in output", async () => {
    const gate = new Montygate();

    const result = await gate.execute("def foo(");
    const output = result.output as { status: string; error: string };
    expect(output.status).toBe("error");
    expect(output.error).toContain("SyntaxError");
    expect(result.stderr).toContain("SyntaxError");
  });
});

describe("e2e: policy enforcement", () => {
  it("denied tool is blocked by policy", async () => {
    const gate = new Montygate({
      policy: {
        defaultAction: "allow",
        rules: [{ matchPattern: "dangerous_*", action: "deny" }],
      },
    });

    gate.tool("dangerous_delete", {
      params: z.object({}),
      run: async () => ({ deleted: true }),
    });

    gate.tool("safe_read", {
      params: z.object({}),
      run: async () => ({ data: "ok" }),
    });

    // The denied tool should cause an error in the script
    const result = await gate.execute(`
try:
    tool('dangerous_delete')
except Exception as e:
    error_msg = str(e)
safe = tool('safe_read')
{"safe": safe, "blocked": error_msg}
    `);

    const output = result.output as { safe: { data: string }; blocked: string };
    expect(output.safe.data).toBe("ok");
    expect(output.blocked).toContain("not allowed");
  });
});

describe("e2e: search and catalog", () => {
  it("search finds tools by name and description", async () => {
    const gate = new Montygate();

    gate.tool("create_invoice", {
      description: "Create a new invoice for billing",
      params: z.object({ amount: z.number() }),
      run: async () => ({}),
    });

    gate.tool("send_notification", {
      description: "Send a push notification",
      params: z.object({ message: z.string() }),
      run: async () => ({}),
    });

    gate.tool("get_billing_history", {
      description: "Get billing history for a customer",
      params: z.object({ customer_id: z.string() }),
      run: async () => ({}),
    });

    // Search by keyword in description
    const billingResults = gate.search("billing");
    expect(billingResults.length).toBeGreaterThanOrEqual(2);
    const names = billingResults.map((r) => r.name);
    expect(names).toContain("create_invoice");
    expect(names).toContain("get_billing_history");

    // Search by name fragment
    const notifResults = gate.search("notification");
    expect(notifResults.length).toBeGreaterThanOrEqual(1);
    expect(notifResults[0].name).toBe("send_notification");
  });

  it("tool catalog includes all registered tools with schemas", async () => {
    const gate = new Montygate();

    gate.tool("add_numbers", {
      description: "Add two numbers",
      params: z.object({ a: z.number(), b: z.number() }),
      run: async ({ a, b }) => a + b,
    });

    const catalog = gate.getToolCatalog();
    expect(catalog).toContain("add_numbers");
    expect(catalog).toContain("Add two numbers");
  });
});

describe("e2e: traces and stats", () => {
  it("traces record timing, inputs, and outputs for each tool call", async () => {
    const gate = new Montygate();

    gate.tool("slow_tool", {
      params: z.object({ delay: z.number() }),
      run: async ({ delay }) => {
        await new Promise((r) => setTimeout(r, delay));
        return { done: true };
      },
    });

    const result = await gate.execute(`
a = tool('slow_tool', delay=50)
b = tool('slow_tool', delay=10)
[a, b]
    `);

    expect(result.trace).toHaveLength(2);

    // Both traces should have timing > 0
    expect(result.trace[0].durationMs).toBeGreaterThan(0);
    expect(result.trace[1].durationMs).toBeGreaterThan(0);

    // Inputs are recorded
    expect(result.trace[0].input).toEqual({ delay: 50 });
    expect(result.trace[1].input).toEqual({ delay: 10 });

    // Outputs are recorded
    expect(result.trace[0].output).toEqual({ done: true });
    expect(result.trace[1].output).toEqual({ done: true });

    // No retries
    expect(result.trace[0].retries).toBe(0);
    expect(result.trace[1].retries).toBe(0);
  });

  it("getTraces() accumulates across executions, clearTraces() resets", async () => {
    const gate = new Montygate();

    gate.tool("ping", {
      params: z.object({}),
      run: async () => "pong",
    });

    await gate.execute("tool('ping')");
    await gate.execute("tool('ping')");

    const traces = gate.getTraces();
    expect(traces).toHaveLength(2);

    gate.clearTraces();
    expect(gate.getTraces()).toHaveLength(0);
  });
});

describe("e2e: adapter round-trip with real engine", () => {
  let gate: Montygate;

  beforeEach(() => {
    gate = new Montygate();

    gate.tool("get_weather", {
      description: "Get weather for a city",
      params: z.object({ city: z.string() }),
      run: async ({ city }) => ({
        city,
        temp: 22,
        condition: "sunny",
      }),
    });
  });

  it("Anthropic adapter: toAnthropic → handleAnthropicToolCall round-trip", async () => {
    const tools = toAnthropic(gate);
    expect(tools).toHaveLength(2);
    expect(tools[0].description).toContain("get_weather");

    // Simulate LLM calling the execute tool
    const result = await handleAnthropicToolCall(gate, "execute", {
      code: "tool('get_weather', city='Paris')",
    });

    expect(result).toEqual({ city: "Paris", temp: 22, condition: "sunny" });
  });

  it("OpenAI adapter: toOpenAI → handleOpenAIToolCall round-trip", async () => {
    const tools = toOpenAI(gate);
    expect(tools[0].function.name).toBe("execute");

    // OpenAI sends args as JSON string
    const result = await handleOpenAIToolCall(
      gate,
      "execute",
      JSON.stringify({ code: "tool('get_weather', city='Tokyo')" }),
    );

    const parsed = JSON.parse(result);
    expect(parsed).toEqual({ city: "Tokyo", temp: 22, condition: "sunny" });
  });

  it("Vercel AI adapter: toVercelAI → execute round-trip", async () => {
    const tools = toVercelAI(gate);

    const result = await tools.execute.execute({
      code: "tool('get_weather', city='London')",
    });

    expect(result).toEqual({ city: "London", temp: 22, condition: "sunny" });
  });

  it("adapter search round-trip with real engine", async () => {
    const searchResult = await handleAnthropicToolCall(gate, "search", {
      query: "weather",
    });

    const results = searchResult as Array<{ name: string }>;
    expect(results.length).toBeGreaterThanOrEqual(1);
    expect(results[0].name).toBe("get_weather");
  });
});

describe("e2e: resource limits", () => {
  it("maxExternalCalls limits tool invocations", async () => {
    const gate = new Montygate({
      resourceLimits: { maxExternalCalls: 2 },
    });

    gate.tool("counter", {
      params: z.object({}),
      run: async () => "ok",
    });

    // Script tries to call 3 tools but limit is 2
    const result = await gate.execute(`
a = tool('counter')
b = tool('counter')
c = tool('counter')
c
    `);

    // Engine returns error in output rather than rejecting
    const output = result.output as { status: string; error: string };
    expect(output.status).toBe("error");
    expect(output.error).toContain("External call limit exceeded");
    // Only 2 calls should have succeeded
    expect(result.trace).toHaveLength(2);
  });
});

describe("e2e: concurrent tool execution", () => {
  it("batch_tools dispatches calls and returns results", async () => {
    const gate = new Montygate({
      limits: { maxConcurrent: 5 },
    });

    const callLog: string[] = [];

    gate.tool("task", {
      params: z.object({ id: z.string() }),
      run: async ({ id }) => {
        callLog.push(`start:${id}`);
        await new Promise((r) => setTimeout(r, 20));
        callLog.push(`end:${id}`);
        return { id, done: true };
      },
    });

    const result = await gate.execute(`
results = batch_tools([
    ('task', {'id': 'A'}),
    ('task', {'id': 'B'}),
    ('task', {'id': 'C'}),
])
[r['id'] for r in results]
    `);

    const output = result.output as string[];
    expect(output).toHaveLength(3);
    expect(output.sort()).toEqual(["A", "B", "C"]);
    expect(result.trace).toHaveLength(3);
  });
});
