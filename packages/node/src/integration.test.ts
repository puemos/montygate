import { describe, it, expect, beforeEach } from "vitest";
import path from "path";

// Load native binding directly for integration testing
// eslint-disable-next-line @typescript-eslint/no-require-imports
const platform = process.platform === "win32" ? "win32" : process.platform;
const arch = process.arch;
const binding = require(
  path.join(__dirname, "..", `montygate.${platform}-${arch}.node`),
);
const NativeEngine = binding.NativeEngine;

describe("NativeEngine integration", () => {
  let engine: InstanceType<typeof NativeEngine>;

  beforeEach(() => {
    engine = new NativeEngine();
  });

  it("creates engine with default config", () => {
    expect(engine.toolCount()).toBe(0);
  });

  it("creates engine with custom config", () => {
    const e = new NativeEngine({
      retry: { maxRetries: 5, baseDelayMs: 200 },
      limits: { timeoutMs: 10000, maxConcurrent: 3 },
    });
    expect(e.toolCount()).toBe(0);
  });

  it("registers a tool", () => {
    engine.registerTool(
      {
        name: "echo",
        description: "Echo input back",
        inputSchema: {
          type: "object",
          properties: { message: { type: "string" } },
          required: ["message"],
        },
      },
      async (args: unknown) => args,
    );
    expect(engine.toolCount()).toBe(1);
  });

  it("generates tool catalog", () => {
    engine.registerTool(
      {
        name: "lookup_order",
        description: "Look up order by ID",
        inputSchema: {
          type: "object",
          properties: { order_id: { type: "string" } },
          required: ["order_id"],
        },
      },
      async () => ({ id: "123" }),
    );

    const catalog = engine.getToolCatalog();
    expect(catalog).toContain("lookup_order");
    expect(catalog).toContain("order_id");
  });

  it("searches tools", () => {
    engine.registerTool(
      {
        name: "create_ticket",
        description: "Create a support ticket",
        inputSchema: { type: "object" },
      },
      async () => ({}),
    );
    engine.registerTool(
      {
        name: "send_email",
        description: "Send an email",
        inputSchema: { type: "object" },
      },
      async () => ({}),
    );

    const results = engine.search("ticket");
    expect(results.length).toBe(1);
    expect(results[0].name).toBe("create_ticket");
  });

  it("executes a simple script", async () => {
    const result = await engine.execute("1 + 2");
    expect(result.output).toBe(3);
    expect(result.trace).toHaveLength(0);
  });

  it("executes a script with inputs", async () => {
    const result = await engine.execute("x + y", { x: 10, y: 20 });
    expect(result.output).toBe(30);
  });

  it("executes a script with tool calls", async () => {
    engine.registerTool(
      {
        name: "double",
        description: "Double a number",
        inputSchema: {
          type: "object",
          properties: { n: { type: "number" } },
        },
      },
      async (args: { n: number }) => args.n * 2,
    );

    const result = await engine.execute("tool('double', n=21)");
    expect(result.output).toBe(42);
    expect(result.trace).toHaveLength(1);
    expect(result.trace[0].toolName).toBe("double");
    expect(result.stats.externalCalls).toBe(1);
  });

  it("executes multi-tool script (token savings demo)", async () => {
    engine.registerTool(
      {
        name: "lookup_order",
        description: "Look up order",
        inputSchema: { type: "object" },
      },
      async () => ({ id: "ORD-123", email: "user@example.com", status: "late" }),
    );
    engine.registerTool(
      {
        name: "create_ticket",
        description: "Create ticket",
        inputSchema: { type: "object" },
      },
      async (args: { subject: string }) => ({
        ticket_id: "TKT-456",
        subject: args.subject,
      }),
    );
    engine.registerTool(
      {
        name: "send_email",
        description: "Send email",
        inputSchema: { type: "object" },
      },
      async () => ({ sent: true }),
    );

    const result = await engine.execute(`
order = tool('lookup_order', order_id='ORD-123')
ticket = tool('create_ticket', subject='Late order ' + order['id'])
tool('send_email', to=order['email'], subject=ticket['subject'])
ticket
    `);

    // Only the final result (ticket) is returned — not intermediate results
    expect(result.output).toEqual({
      ticket_id: "TKT-456",
      subject: "Late order ORD-123",
    });
    // 3 tool calls happened
    expect(result.trace).toHaveLength(3);
    expect(result.stats.externalCalls).toBe(3);
  });

  it("records and clears traces", async () => {
    engine.registerTool(
      {
        name: "ping",
        description: "Ping",
        inputSchema: { type: "object" },
      },
      async () => "pong",
    );

    await engine.execute("tool('ping')");
    const traces = engine.getTraces();
    expect(traces.length).toBeGreaterThan(0);
    expect(traces[0].toolName).toBe("ping");

    engine.clearTraces();
    expect(engine.getTraces()).toHaveLength(0);
  });

  it("handles tool errors gracefully", async () => {
    engine.registerTool(
      {
        name: "fail_tool",
        description: "Always fails",
        inputSchema: { type: "object" },
      },
      async () => {
        throw new Error("tool broke");
      },
    );

    // The script should get an error back from the tool
    const result = await engine.execute(`
try:
    tool('fail_tool')
except Exception as e:
    str(e)
    `);
    // Result should contain error info
    expect(result.output).toBeDefined();
  });
});
