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

  it("tool catalog includes all registered tools with schemas", async () => {
    const gate = new Montygate();

    gate.tool("add_numbers", {
      description: "Add two numbers",
      params: z.object({ a: z.number(), b: z.number() }),
      run: async ({ a, b }) => a + b,
    });

    gate.tool("create_ticket", {
      description: "Create a customer support ticket",
      params: z.object({
        subject: z.string().describe("Ticket subject line"),
        priority: z.enum(["low", "medium", "high"]).describe("Ticket priority"),
      }),
      returns: z.object({ ticket_id: z.string() }),
      run: async () => ({ ticket_id: "TKT-1" }),
    });

    const catalog = gate.getToolCatalog();
    expect(catalog).toContain("add_numbers");
    expect(catalog).toContain("Add two numbers");
    expect(catalog).toContain('priority: string ("low"|"medium"|"high")');
    expect(catalog).toContain("subject: string (Ticket subject line)");
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

  it("gate.anthropic() → gate.handleToolCall() round-trip", async () => {
    const tools = gate.anthropic();
    expect(tools).toHaveLength(2);
    expect(tools[0].description).toContain("get_weather");

    const result = await gate.handleToolCall("execute", {
      code: "tool('get_weather', city='Paris')",
    });

    expect(result).toEqual({ city: "Paris", temp: 22, condition: "sunny" });
  });

  it("gate.openai() → gate.handleToolCall() round-trip (JSON string args)", async () => {
    const tools = gate.openai();
    expect(tools[0].function.name).toBe("execute");

    const result = await gate.handleToolCall(
      "execute",
      JSON.stringify({ code: "tool('get_weather', city='Tokyo')" }),
    );

    expect(result).toEqual({ city: "Tokyo", temp: 22, condition: "sunny" });
  });

  it("gate.vercelai() → execute round-trip", async () => {
    const tools = gate.vercelai();

    const result = await tools.execute.execute({
      code: "tool('get_weather', city='London')",
    });

    expect(result).toEqual({ city: "London", temp: 22, condition: "sunny" });
  });

  it("handleToolCall search round-trip with real engine", async () => {
    const results = await gate.handleToolCall("search", {
      query: "weather",
    });

    const hits = results as Array<{ name: string }>;
    expect(hits.length).toBeGreaterThanOrEqual(1);
    expect(hits[0].name).toBe("get_weather");
  });
});

describe("e2e: prompt guidance", () => {
  it("system prompt includes good and bad execute examples", () => {
    const gate = new Montygate();
    const prompt = gate.getSystemPrompt();
    expect(prompt).toContain("GOOD:");
    expect(prompt).toContain("BAD:");
    expect(prompt).toContain("NameError");
  });

  it("system prompt mentions fresh sandbox and single script", () => {
    const gate = new Montygate();
    const prompt = gate.getSystemPrompt();
    expect(prompt).toContain("FRESH sandbox");
    expect(prompt).toContain("SINGLE script");
    expect(prompt).toContain("batch_tools()");
    expect(prompt).toContain("LAST EXPRESSION");
  });

  it("system prompt is identical across instances", () => {
    const gate1 = new Montygate();
    const gate2 = new Montygate();
    gate2.tool("some_tool", {
      params: z.object({ x: z.string() }),
      run: async () => "ok",
    });
    // System prompt should not change based on registered tools
    expect(gate1.getSystemPrompt()).toBe(gate2.getSystemPrompt());
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
    expect(props.inputs.type).toBe("object");
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

    // Anthropic input_schema should be the same object structure
    expect(anthropicTools[0].input_schema).toEqual(centralSchema);
    // OpenAI parameters should be the same object structure
    expect(openaiTools[0].function.parameters).toEqual(centralSchema);
  });

  it("adapter schemas match the centralized search schema", () => {
    const gate = new Montygate();
    const centralSchema = gate.getSearchToolInputSchema();
    const anthropicTools = gate.anthropic();
    const openaiTools = gate.openai();

    expect(anthropicTools[1].input_schema).toEqual(centralSchema);
    expect(openaiTools[1].function.parameters).toEqual(centralSchema);
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
});
