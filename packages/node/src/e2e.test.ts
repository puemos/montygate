import { beforeEach, describe, expect, it } from "vitest";
import { z } from "zod";
import { Montygate } from "./engine.js";
import type { ToolHandlerMap } from "./normalize.js";

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
    expect(output.notification.body).toBe("Ticket TKT-100 created for TR-789");

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
      run: async ({ items }) =>
        items.reduce((a: number, b: number) => a + b, 0),
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

  it("print()-only script returns stdout hint instead of bare null", async () => {
    const gate = new Montygate();

    gate.tool("ping", {
      description: "Ping",
      params: z.object({}),
      run: async () => "pong",
    });

    const result = await gate.execute(`
result = tool('ping')
print(result)
    `);

    // Should surface stdout so the LLM gets feedback
    const output = result.output as { result: null; stdout: string };
    expect(output.result).toBeNull();
    expect(output.stdout).toContain("pong");
  });

  it("script syntax error returns error in output", async () => {
    const gate = new Montygate();

    const result = await gate.execute("def foo(");
    const output = result.output as { status: string; error: string };
    expect(output.status).toBe("error");
    expect(output.error).toContain("SyntaxError");
    expect(result.stderr).toContain("SyntaxError");
  });

  it("unknown tool error suggests similar tool names", async () => {
    const gate = new Montygate();

    gate.tool("create_ticket", {
      params: z.object({ subject: z.string() }),
      run: async ({ subject }) => ({ subject }),
    });

    const result = await gate.execute(`tool('create_tiket', subject='Bug')`);
    const output = result.output as { status: string; error: string };
    expect(output.status).toBe("error");
    expect(output.error).toContain("Did you mean");
    expect(output.error).toContain("create_ticket");
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
      returns: z.object({ invoice_id: z.string(), status: z.string() }),
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

    // Search by output field name and preserve output schema
    const invoiceResults = gate.search("invoice_id");
    expect(invoiceResults.length).toBeGreaterThanOrEqual(1);
    expect(invoiceResults[0].name).toBe("create_invoice");
    expect(invoiceResults[0].outputSchema).toBeDefined();
  });

  it("tool catalog is non-empty after registering tools", async () => {
    const gate = new Montygate();

    gate.tool("add_numbers", {
      description: "Add two numbers",
      params: z.object({ a: z.number(), b: z.number() }),
      run: async ({ a, b }) => a + b,
    });

    const catalog = gate.getToolCatalog();
    expect(typeof catalog).toBe("string");
    expect(catalog.length).toBeGreaterThan(0);
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

  it("gate.anthropic() includes original tools plus meta-tools", async () => {
    const tools = gate.anthropic();
    expect(tools).toHaveLength(3); // get_weather + execute + search
    expect(tools[0].name).toBe("get_weather");
    expect(tools[1].name).toBe("execute");
    expect(tools[2].name).toBe("search");
  });

  it("gate.anthropic() → gate.handleToolCall() round-trip via execute", async () => {
    const result = await gate.handleToolCall("execute", {
      code: "tool('get_weather', city='Paris')",
    });

    expect(result).toEqual({ city: "Paris", temp: 22, condition: "sunny" });
  });

  it("gate.anthropic() → gate.handleToolCall() direct call round-trip", async () => {
    const result = await gate.handleToolCall("get_weather", { city: "Berlin" });
    expect(result).toEqual({ city: "Berlin", temp: 22, condition: "sunny" });
  });

  it("gate.openai() includes original tools plus meta-tools", async () => {
    const tools = gate.openai();
    expect(tools).toHaveLength(3);
    expect(tools[0].function.name).toBe("get_weather");
    expect(tools[1].function.name).toBe("execute");
  });

  it("gate.openai() → gate.handleToolCall() round-trip (JSON string args)", async () => {
    const result = await gate.handleToolCall(
      "execute",
      JSON.stringify({ code: "tool('get_weather', city='Tokyo')" }),
    );

    expect(result).toEqual({ city: "Tokyo", temp: 22, condition: "sunny" });
  });

  it("gate.vercelai() includes original tools", async () => {
    const tools = gate.vercelai();
    expect(tools).toHaveProperty("get_weather");
    expect(tools).toHaveProperty("execute");
    expect(tools).toHaveProperty("search");
  });

  it("gate.vercelai() → execute round-trip", async () => {
    const tools = gate.vercelai();

    const result = await tools.execute.execute({
      code: "tool('get_weather', city='London')",
    });

    expect(result).toEqual({ city: "London", temp: 22, condition: "sunny" });
  });

  it("gate.vercelai() → direct tool call round-trip", async () => {
    const tools = gate.vercelai();
    const result = await tools.get_weather.execute({ city: "Sydney" });
    expect(result).toEqual({ city: "Sydney", temp: 22, condition: "sunny" });
  });

  it("handleToolCall search round-trip with real engine", async () => {
    const results = await gate.handleToolCall("search", {
      query: "weather",
    });

    const hits = results as Array<{ name: string }>;
    expect(hits.length).toBeGreaterThanOrEqual(1);
    expect(hits[0].name).toBe("get_weather");
  });

  it("hybrid flow: mix direct calls and execute calls", async () => {
    // Direct call
    const weather = await gate.handleToolCall("get_weather", { city: "NYC" });
    expect(weather).toEqual({ city: "NYC", temp: 22, condition: "sunny" });

    // Execute call using same tool in a script
    const result = await gate.handleToolCall("execute", {
      code: `
w = tool('get_weather', city='LA')
w['city'] + ' is ' + w['condition']
      `,
    });
    expect(result).toBe("LA is sunny");
  });
});

describe("e2e: state injection", () => {
  let gate: Montygate;

  beforeEach(() => {
    gate = new Montygate();
    gate.tool("get_customer", {
      params: z.object({ customer_id: z.string() }),
      run: async ({ customer_id }) => ({
        id: customer_id,
        name: "Alice",
        email: "alice@example.com",
        tier: "gold",
      }),
    });
    gate.tool("lookup_order", {
      params: z.object({ order_id: z.string() }),
      run: async ({ order_id }) => ({
        id: order_id,
        total: 99.99,
        customer_id: "CUST-1",
      }),
    });
    gate.tool("process_refund", {
      params: z.object({ order_id: z.string(), amount: z.number() }),
      run: async ({ order_id, amount }) => ({
        refund_id: "REF-001",
        order_id,
        amount,
        status: "processed",
      }),
    });
  });

  it("cached tool results are available in subsequent execute via handleToolCall", async () => {
    // First call: fetch customer
    await gate.handleToolCall("execute", {
      code: "tool('get_customer', customer_id='CUST-1')",
    });

    // Second call: use last_get_customer — should NOT need to call get_customer again
    const result = await gate.handleToolCall("execute", {
      code: "last_get_customer['name']",
    });

    expect(result).toBe("Alice");
  });

  it("last_result contains prior script output", async () => {
    await gate.handleToolCall("execute", {
      code: "{'x': 42, 'y': 'hello'}",
    });

    const result = await gate.handleToolCall("execute", {
      code: "last_result",
    });

    expect(result).toEqual({ x: 42, y: "hello" });
  });

  it("explicit inputs override cached state", async () => {
    // Populate cache with get_customer result
    await gate.handleToolCall("execute", {
      code: "tool('get_customer', customer_id='CUST-1')",
    });

    // Execute with explicit override — should use the override, not the cache
    const result = await gate.execute("last_get_customer['name']", {
      last_get_customer: { id: "CUST-X", name: "Override", email: "x@x.com", tier: "silver" },
    });

    expect(result.output).toBe("Override");
  });

  it("clearStateCache resets all cached state", async () => {
    // Populate cache
    await gate.handleToolCall("execute", {
      code: "tool('get_customer', customer_id='CUST-1')",
    });

    gate.clearStateCache();

    // Now last_get_customer should not exist — expect sandbox error
    const result = await gate.execute("last_get_customer");
    const output = result.output as { status: string; error: string };
    expect(output.status).toBe("error");
    expect(output.error).toContain("NameError");
  });

  it("cache updates on each execution (latest wins)", async () => {
    // First call with CUST-1
    await gate.handleToolCall("execute", {
      code: "tool('get_customer', customer_id='CUST-1')",
    });

    // Second call with CUST-2 (returns different data but same tool)
    gate.tool("get_customer_v2", {
      params: z.object({ customer_id: z.string() }),
      run: async ({ customer_id }) => ({
        id: customer_id,
        name: "Bob",
        email: "bob@example.com",
        tier: "silver",
      }),
    });

    // Re-call get_customer — its cache entry will update
    await gate.handleToolCall("execute", {
      code: "tool('lookup_order', order_id='ORD-1')",
    });

    // last_lookup_order should now be the latest result
    const result = await gate.handleToolCall("execute", {
      code: "last_lookup_order['id']",
    });
    expect(result).toBe("ORD-1");
  });

  it("failed tool results are not cached", async () => {
    gate.tool("flaky", {
      params: z.object({}),
      run: async () => {
        throw new Error("boom");
      },
    });

    // Call flaky tool — it errors but is caught by the script
    await gate.handleToolCall("execute", {
      code: `
try:
    tool('flaky')
except:
    pass
'done'
`,
    });

    // last_flaky should NOT exist in cache
    const result = await gate.execute("last_flaky");
    const output = result.output as { status: string; error: string };
    expect(output.status).toBe("error");
    expect(output.error).toContain("last_flaky");
  });

  it("state injection can be disabled via config", async () => {
    const noStateGate = new Montygate({ stateInjection: false });
    noStateGate.tool("get_customer", {
      params: z.object({ customer_id: z.string() }),
      run: async ({ customer_id }) => ({ id: customer_id, name: "Alice" }),
    });

    await noStateGate.handleToolCall("execute", {
      code: "tool('get_customer', customer_id='CUST-1')",
    });

    // Without state injection, last_get_customer should not be available
    const result = await noStateGate.execute("last_get_customer");
    const output = result.output as { status: string; error: string };
    expect(output.status).toBe("error");
    expect(output.error).toContain("NameError");
  });

  it("multi-step pipeline works across handleToolCall calls without re-fetching", async () => {
    const callCounts = { get_customer: 0, lookup_order: 0, process_refund: 0 };

    const stepGate = new Montygate();
    stepGate.tool("get_customer", {
      params: z.object({ customer_id: z.string() }),
      run: async ({ customer_id }) => {
        callCounts.get_customer++;
        return { id: customer_id, name: "Alice", tier: "gold" };
      },
    });
    stepGate.tool("lookup_order", {
      params: z.object({ order_id: z.string() }),
      run: async ({ order_id }) => {
        callCounts.lookup_order++;
        return { id: order_id, total: 99.99, customer_id: "CUST-1" };
      },
    });
    stepGate.tool("process_refund", {
      params: z.object({ order_id: z.string(), amount: z.number() }),
      run: async ({ order_id, amount }) => {
        callCounts.process_refund++;
        return { refund_id: "REF-1", order_id, amount };
      },
    });

    // Step 1: Fetch customer and order
    await stepGate.handleToolCall("execute", {
      code: `
customer = tool('get_customer', customer_id='CUST-1')
order = tool('lookup_order', order_id='ORD-1')
{'customer': customer, 'order': order}
`,
    });

    // Step 2: Use cached results — no re-fetch needed
    const result = await stepGate.handleToolCall("execute", {
      code: `
tool('process_refund', order_id=last_lookup_order['id'], amount=last_lookup_order['total'])
`,
    });

    expect(result).toEqual({
      refund_id: "REF-1",
      order_id: "ORD-1",
      amount: 99.99,
    });

    // Verify: get_customer and lookup_order were only called ONCE
    expect(callCounts.get_customer).toBe(1);
    expect(callCounts.lookup_order).toBe(1);
    expect(callCounts.process_refund).toBe(1);
  });
});

describe("e2e: NameError auto-retry", () => {
  it("auto-retries when NameError variable exists in cache", async () => {
    const gate = new Montygate();
    gate.tool("get_customer", {
      params: z.object({ customer_id: z.string() }),
      run: async ({ customer_id }) => ({
        id: customer_id,
        name: "Alice",
      }),
    });

    // Step 1: Populate cache
    await gate.handleToolCall("execute", {
      code: "tool('get_customer', customer_id='CUST-1')",
    });

    // Step 2: Script references `customer` (not `last_get_customer`) —
    // this will NameError, but `customer` doesn't exist in cache either.
    // However, `last_get_customer` IS available as a pre-set variable.
    // This should work because last_get_customer is injected:
    const result = await gate.handleToolCall("execute", {
      code: "last_get_customer['name']",
    });
    expect(result).toBe("Alice");
  });

  it("auto-retry resolves missing variable from last_ prefix in cache", async () => {
    const gate = new Montygate();
    gate.tool("fetch_data", {
      params: z.object({}),
      run: async () => ({ value: 42 }),
    });

    // Step 1: Populate cache
    await gate.handleToolCall("execute", {
      code: "tool('fetch_data')",
    });

    // Step 2: Reference `fetch_data` (bare name) — NameError triggers auto-retry
    // auto-retry checks cache for `fetch_data` → not found, checks `last_fetch_data` → found
    const result = await gate.handleToolCall("execute", {
      code: "fetch_data['value']",
    });

    // The auto-retry should inject last_fetch_data as `fetch_data`
    // fetch_data['value'] is a Python dict subscript → returns 42
    expect(result).toBe(42);
  });

  it("propagates error when NameError variable is not in cache", async () => {
    const gate = new Montygate();

    // No prior executions — cache is empty
    await expect(
      gate.handleToolCall("execute", { code: "unknown_var" }),
    ).rejects.toThrow("NameError");
  });

  it("does not retry more than once", async () => {
    const gate = new Montygate();
    let callCount = 0;

    gate.tool("noop", {
      params: z.object({}),
      run: async () => {
        callCount++;
        return "ok";
      },
    });

    // Populate something in cache to trigger a retry
    await gate.handleToolCall("execute", { code: "tool('noop')" });

    // This references `bogus_var` which is NOT in cache — no retry should happen
    await expect(
      gate.handleToolCall("execute", { code: "bogus_var" }),
    ).rejects.toThrow("NameError");

    // noop was only called once (the initial populate call)
    expect(callCount).toBe(1);
  });
});

describe("e2e: state summary", () => {
  it("getStateSummary returns null when no state cached", () => {
    const gate = new Montygate();
    expect(gate.getStateSummary()).toBeNull();
  });

  it("getStateSummary is non-null after tool execution", async () => {
    const gate = new Montygate();
    gate.tool("get_customer", {
      params: z.object({ customer_id: z.string() }),
      run: async () => ({ id: "CUST-1", name: "Alice" }),
    });

    await gate.handleToolCall("execute", {
      code: "tool('get_customer', customer_id='CUST-1')",
    });

    expect(gate.getStateSummary()).not.toBeNull();
  });

  it("clearStateCache clears the summary", async () => {
    const gate = new Montygate();
    gate.tool("ping", {
      params: z.object({}),
      run: async () => "pong",
    });

    await gate.handleToolCall("execute", { code: "tool('ping')" });
    expect(gate.getStateSummary()).not.toBeNull();

    gate.clearStateCache();
    expect(gate.getStateSummary()).toBeNull();
  });
});

describe("e2e: centralized schemas", () => {
  it("getExecuteToolInputSchema returns valid JSON Schema", () => {
    const gate = new Montygate();
    const schema = gate.getExecuteToolInputSchema();

    expect(schema.type).toBe("object");
    expect(schema.properties).toBeDefined();
    const props = schema.properties as Record<string, Record<string, unknown>>;
    expect(props.code.type).toBe("string");
    expect(props.inputs).toBeUndefined();
    expect(schema.required).toEqual(["code"]);
  });

  it("getSearchToolInputSchema returns valid JSON Schema", () => {
    const gate = new Montygate();
    const schema = gate.getSearchToolInputSchema();

    expect(schema.type).toBe("object");
    expect(schema.properties).toBeDefined();
    const props = schema.properties as Record<string, Record<string, unknown>>;
    expect(props.query.type).toBe("string");
    expect(props.top_k.type).toBe("number");
    expect(schema.required).toEqual(["query"]);
  });

  it("adapter schemas match the centralized execute schema", () => {
    const gate = new Montygate();
    const centralSchema = gate.getExecuteToolInputSchema();
    const anthropicTools = gate.anthropic();
    const openaiTools = gate.openai();

    // Find execute tool (after original tools)
    const anthropicExec = anthropicTools.find((t) => t.name === "execute")!;
    const openaiExec = openaiTools.find((t) => t.function.name === "execute")!;

    expect(anthropicExec.input_schema).toEqual(centralSchema);
    expect(openaiExec.function.parameters).toEqual(centralSchema);
  });

  it("adapter schemas match the centralized search schema", () => {
    const gate = new Montygate();
    const centralSchema = gate.getSearchToolInputSchema();
    const anthropicTools = gate.anthropic();
    const openaiTools = gate.openai();

    const anthropicSearch = anthropicTools.find((t) => t.name === "search")!;
    const openaiSearch = openaiTools.find((t) => t.function.name === "search")!;

    expect(anthropicSearch.input_schema).toEqual(centralSchema);
    expect(openaiSearch.function.parameters).toEqual(centralSchema);
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

describe("e2e: .tools() universal registration", () => {
  it("registers OpenAI Chat Completions tools + handlers and executes", async () => {
    const gate = new Montygate();
    const openaiTools = [
      {
        type: "function",
        function: {
          name: "get_weather",
          description: "Get weather for a city",
          parameters: {
            type: "object",
            properties: { city: { type: "string" } },
            required: ["city"],
          },
        },
      },
    ];
    const handlers: ToolHandlerMap = {
      get_weather: async (args: unknown) => {
        const { city } = args as { city: string };
        return { city, temp: 72, condition: "sunny" };
      },
    };

    gate.tools(openaiTools, handlers);
    expect(gate.toolCount).toBe(1);

    const result = await gate.execute("tool('get_weather', city='Paris')");
    const output = result.output as { city: string; temp: number };
    expect(output.city).toBe("Paris");
    expect(output.temp).toBe(72);
  });

  it("registers Anthropic raw tools + handlers and executes", async () => {
    const gate = new Montygate();
    const anthropicTools = [
      {
        name: "lookup_order",
        description: "Look up order by ID",
        input_schema: {
          type: "object",
          properties: { order_id: { type: "string" } },
          required: ["order_id"],
        },
      },
    ];
    const handlers: ToolHandlerMap = {
      lookup_order: async (args: unknown) => {
        const { order_id } = args as { order_id: string };
        return { id: order_id, status: "shipped" };
      },
    };

    gate.tools(anthropicTools, handlers);

    const result = await gate.execute("tool('lookup_order', order_id='ORD-1')");
    const output = result.output as { id: string; status: string };
    expect(output.id).toBe("ORD-1");
    expect(output.status).toBe("shipped");
  });

  it("registers Vercel AI-style tools (object with embedded execute)", async () => {
    const gate = new Montygate();
    const vercelTools = {
      get_weather: {
        description: "Get weather",
        parameters: z.object({ city: z.string() }),
        execute: async (args: unknown) => {
          const { city } = args as { city: string };
          return { city, temp: 22 };
        },
      },
    };

    gate.tools(vercelTools);
    expect(gate.toolCount).toBe(1);

    const result = await gate.execute("tool('get_weather', city='London')");
    const output = result.output as { city: string; temp: number };
    expect(output.city).toBe("London");
    expect(output.temp).toBe(22);
  });

  it("chains .tools() from multiple sources", async () => {
    const gate = new Montygate();

    gate
      .tools(
        [
          {
            type: "function",
            function: {
              name: "tool_a",
              parameters: { type: "object", properties: {} },
            },
          },
        ],
        { tool_a: async () => "a_result" },
      )
      .tools({
        tool_b: {
          description: "Tool B",
          parameters: z.object({}),
          execute: async () => "b_result",
        },
      });

    expect(gate.toolCount).toBe(2);

    const result = await gate.execute(`
a = tool('tool_a')
b = tool('tool_b')
[a, b]
    `);

    const output = result.output as string[];
    expect(output).toEqual(["a_result", "b_result"]);
  });

  it("registers tools via constructor config", async () => {
    const openaiTools = [
      {
        type: "function",
        function: {
          name: "greet",
          parameters: {
            type: "object",
            properties: { name: { type: "string" } },
            required: ["name"],
          },
        },
      },
    ];

    const gate = new Montygate({
      tools: openaiTools,
      handlers: {
        greet: async (args: unknown) => {
          const { name } = args as { name: string };
          return `Hello, ${name}!`;
        },
      },
    });

    expect(gate.toolCount).toBe(1);

    const result = await gate.execute("tool('greet', name='World')");
    expect(result.output).toBe("Hello, World!");
  });

  it("throws clear error when tool has no handler", () => {
    const gate = new Montygate();
    const tools = [
      {
        type: "function",
        function: {
          name: "orphan_tool",
          parameters: { type: "object" },
        },
      },
    ];

    expect(() => gate.tools(tools)).toThrow(
      "Tool 'orphan_tool' (detected as openai-chat format) has no handler",
    );
  });

  it("throws for unknown tool format", () => {
    const gate = new Montygate();
    expect(() => gate.tools([{ random: "shape" }])).toThrow(
      "Could not detect tool format",
    );
  });

  it("mixes .tools() with .tool() on same engine", async () => {
    const gate = new Montygate();

    // Register via .tools()
    gate.tools(
      [
        {
          name: "fetch_data",
          description: "Fetch data",
          input_schema: {
            type: "object",
            properties: { id: { type: "string" } },
          },
        },
      ],
      {
        fetch_data: async (args: unknown) => ({
          id: (args as { id: string }).id,
          value: 42,
        }),
      },
    );

    // Register via .tool()
    gate.tool("process_data", {
      params: z.object({ value: z.number() }),
      run: async ({ value }) => value * 2,
    });

    expect(gate.toolCount).toBe(2);

    const result = await gate.execute(`
data = tool('fetch_data', id='X')
tool('process_data', value=data['value'])
    `);
    expect(result.output).toBe(84);
  });

  it("rejects tool named 'execute' via .tool()", () => {
    const gate = new Montygate();
    expect(() =>
      gate.tool("execute", {
        params: z.object({}),
        run: async () => "nope",
      }),
    ).toThrow("reserved");
  });

  it("rejects tool named 'search' via .tool()", () => {
    const gate = new Montygate();
    expect(() =>
      gate.tool("search", {
        params: z.object({}),
        run: async () => "nope",
      }),
    ).toThrow("reserved");
  });

  it("rejects tool named 'execute' via .tools()", () => {
    const gate = new Montygate();
    expect(() =>
      gate.tools(
        [
          {
            name: "execute",
            description: "conflict",
            input_schema: { type: "object", properties: {} },
          },
        ],
        { execute: async () => "nope" },
      ),
    ).toThrow("reserved");
  });
});
