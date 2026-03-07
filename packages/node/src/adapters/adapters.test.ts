import { beforeEach, describe, expect, it, vi } from "vitest";
import { Montygate } from "../engine.js";
import { buildTraceSummary, unwrapExecutionResult } from "./utils.js";

function createMockEngine(): Montygate {
  const catalog = "- lookup_order(order_id: string) - Look up an order\n";
  const engine = {
    getToolCatalog: vi.fn(() => catalog),
    getExecuteToolDescription: vi.fn(
      () =>
        `Execute a Python script with access to these tools:\n${catalog}\nCall tools with: tool('name', key=value)\nThe LAST EXPRESSION is the return value. Do NOT use print() — it returns None.\n\nRuntime restrictions:\n- No standard library\n\nExample:\norder = tool('lookup_order', order_id='123')`,
    ),
    getSearchToolDescription: vi.fn(
      () => "Search for available tools by keyword",
    ),
    getExecuteToolInputSchema: vi.fn(() => ({
      type: "object",
      properties: {
        code: { type: "string", description: "Python script to execute" },
        inputs: {
          type: "object",
          description: "Variables to inject into the script",
        },
      },
      required: ["code"],
    })),
    getSearchToolInputSchema: vi.fn(() => ({
      type: "object",
      properties: {
        query: { type: "string", description: "Search query" },
        top_k: { type: "number", description: "Maximum number of results" },
      },
      required: ["query"],
    })),
    execute: vi.fn(async () => ({
      output: { id: "123", status: "shipped" },
      stdout: "",
      stderr: "",
      trace: [],
      stats: {
        totalDurationMs: 42,
        montyExecutionMs: 10,
        externalCalls: 1,
        memoryPeakBytes: 0,
        stepsExecuted: 0,
      },
    })),
    search: vi.fn(() => [
      {
        name: "lookup_order",
        description: "Look up an order",
        inputSchema: { type: "object" },
      },
    ]),
    toolCount: 1,
    getTools: vi.fn(() => []),
    getTraces: vi.fn(() => []),
    clearTraces: vi.fn(),
  };
  // Bind real class methods to the mock so .anthropic(), .openai(), etc. work
  engine.anthropic = Montygate.prototype.anthropic.bind(engine);
  engine.openai = Montygate.prototype.openai.bind(engine);
  engine.vercelai = Montygate.prototype.vercelai.bind(engine);
  engine.handleToolCall = Montygate.prototype.handleToolCall.bind(engine);
  return engine as unknown as Montygate;
}

describe("gate.anthropic()", () => {
  let engine: Montygate;

  beforeEach(() => {
    engine = createMockEngine();
  });

  it("returns execute and search tools", () => {
    const tools = engine.anthropic();
    expect(tools).toHaveLength(2);
    expect(tools[0].name).toBe("execute");
    expect(tools[1].name).toBe("search");
  });

  it("execute tool includes tool catalog and usage instructions in description", () => {
    const tools = engine.anthropic();
    expect(tools[0].description).toContain("lookup_order");
    expect(tools[0].description).toContain("LAST EXPRESSION");
    expect(tools[0].description).toContain("Do NOT use print()");
  });

  it("execute tool has correct input_schema", () => {
    const tools = engine.anthropic();
    expect(tools[0].input_schema.type).toBe("object");
    expect(tools[0].input_schema.properties).toHaveProperty("code");
    expect(tools[0].input_schema.required).toContain("code");
  });

  it("search tool has correct input_schema", () => {
    const tools = engine.anthropic();
    expect(tools[1].input_schema.properties).toHaveProperty("query");
    expect(tools[1].input_schema.required).toContain("query");
  });

  it("reads schemas from engine instead of hardcoding", () => {
    engine.anthropic();
    expect(engine.getExecuteToolInputSchema).toHaveBeenCalled();
    expect(engine.getSearchToolInputSchema).toHaveBeenCalled();
  });
});

describe("gate.openai()", () => {
  let engine: Montygate;

  beforeEach(() => {
    engine = createMockEngine();
  });

  it("returns function type tools", () => {
    const tools = engine.openai();
    expect(tools).toHaveLength(2);
    expect(tools[0].type).toBe("function");
    expect(tools[0].function.name).toBe("execute");
    expect(tools[1].type).toBe("function");
    expect(tools[1].function.name).toBe("search");
  });

  it("execute tool has parameters", () => {
    const tools = engine.openai();
    expect(tools[0].function.parameters.type).toBe("object");
    expect(tools[0].function.parameters.properties).toHaveProperty("code");
  });

  it("reads schemas from engine instead of hardcoding", () => {
    engine.openai();
    expect(engine.getExecuteToolInputSchema).toHaveBeenCalled();
    expect(engine.getSearchToolInputSchema).toHaveBeenCalled();
  });
});

describe("gate.vercelai()", () => {
  let engine: Montygate;

  beforeEach(() => {
    engine = createMockEngine();
  });

  it("returns execute and search tools", () => {
    const tools = engine.vercelai();
    expect(tools).toHaveProperty("execute");
    expect(tools).toHaveProperty("search");
  });

  it("execute tool has description with instructions and parameters", () => {
    const tools = engine.vercelai();
    expect(tools.execute.description).toContain("lookup_order");
    expect(tools.execute.description).toContain("LAST EXPRESSION");
    expect(tools.execute.parameters).toBeDefined();
  });

  it("execute tool dispatches correctly", async () => {
    const tools = engine.vercelai();
    const result = await tools.execute.execute({ code: "42" });
    expect(result).toEqual({ id: "123", status: "shipped" });
  });

  it("search tool dispatches correctly", async () => {
    const tools = engine.vercelai();
    const result = await tools.search.execute({ query: "order" });
    expect(result).toHaveLength(1);
  });
});

describe("gate.handleToolCall()", () => {
  let engine: Montygate;

  beforeEach(() => {
    engine = createMockEngine();
  });

  it("dispatches execute call with object args", async () => {
    const result = await engine.handleToolCall("execute", {
      code: "tool('lookup_order', order_id='123')",
    });
    expect(result).toEqual({ id: "123", status: "shipped" });
    expect(engine.execute).toHaveBeenCalledWith(
      "tool('lookup_order', order_id='123')",
      undefined,
    );
  });

  it("dispatches execute call with JSON string args (OpenAI style)", async () => {
    const result = await engine.handleToolCall(
      "execute",
      JSON.stringify({ code: "42" }),
    );
    expect(result).toEqual({ id: "123", status: "shipped" });
  });

  it("dispatches search call", async () => {
    const result = await engine.handleToolCall("search", {
      query: "order",
    });
    expect(result).toHaveLength(1);
    expect(engine.search).toHaveBeenCalledWith("order", undefined);
  });

  it("throws for unknown tool", async () => {
    await expect(
      engine.handleToolCall("unknown", {}),
    ).rejects.toThrow("Unknown tool: unknown");
  });
});

describe("unwrapExecutionResult", () => {
  it("returns output for successful results", () => {
    const result = unwrapExecutionResult({
      output: { id: "123", status: "shipped" },
      stdout: "",
      stderr: "",
      trace: [],
      stats: {
        totalDurationMs: 0,
        montyExecutionMs: 0,
        externalCalls: 0,
        memoryPeakBytes: 0,
        stepsExecuted: 0,
      },
    });
    expect(result).toEqual({ id: "123", status: "shipped" });
  });

  it("throws on sandbox error output", () => {
    expect(() =>
      unwrapExecutionResult({
        output: {
          status: "error",
          error: "NameError: name 'order' is not defined",
        },
        stdout: "",
        stderr: "",
        trace: [],
        stats: {
          totalDurationMs: 0,
          montyExecutionMs: 0,
          externalCalls: 0,
          memoryPeakBytes: 0,
          stepsExecuted: 0,
        },
      }),
    ).toThrow("NameError");
  });

  it("includes fresh sandbox hint in error message", () => {
    expect(() =>
      unwrapExecutionResult({
        output: {
          status: "error",
          error: "NameError: name 'x' is not defined",
        },
        stdout: "",
        stderr: "",
        trace: [],
        stats: {
          totalDurationMs: 0,
          montyExecutionMs: 0,
          externalCalls: 0,
          memoryPeakBytes: 0,
          stepsExecuted: 0,
        },
      }),
    ).toThrow("fresh sandbox");
  });

  it("includes prior successful tool outputs in error messages", () => {
    expect(() =>
      unwrapExecutionResult({
        output: {
          status: "error",
          error: "NameError: name 'ticket' is not defined",
        },
        stdout: "",
        stderr: "",
        trace: [
          {
            toolName: "lookup_order",
            input: { order_id: "ORD-123" },
            output: { id: "ORD-123", email: "alice@example.com" },
            durationMs: 0,
            retries: 0,
          },
          {
            toolName: "create_ticket",
            input: { subject: "Late order" },
            output: { ticket_id: "TKT-100", status: "open" },
            durationMs: 0,
            retries: 0,
          },
        ],
        stats: {
          totalDurationMs: 0,
          montyExecutionMs: 0,
          externalCalls: 0,
          memoryPeakBytes: 0,
          stepsExecuted: 0,
        },
      }),
    ).toThrow('create_ticket OK -> {"ticket_id":"TKT-100","status":"open"}');
  });

  it("does not throw for non-error objects", () => {
    const result = unwrapExecutionResult({
      output: { status: "ok", data: "hello" },
      stdout: "",
      stderr: "",
      trace: [],
      stats: {
        totalDurationMs: 0,
        montyExecutionMs: 0,
        externalCalls: 0,
        memoryPeakBytes: 0,
        stepsExecuted: 0,
      },
    });
    expect(result).toEqual({ status: "ok", data: "hello" });
  });

  it("does not throw for null output", () => {
    const result = unwrapExecutionResult({
      output: null,
      stdout: "",
      stderr: "",
      trace: [],
      stats: {
        totalDurationMs: 0,
        montyExecutionMs: 0,
        externalCalls: 0,
        memoryPeakBytes: 0,
        stepsExecuted: 0,
      },
    });
    expect(result).toBeNull();
  });
});

describe("buildTraceSummary", () => {
  it("returns null for empty traces", () => {
    expect(buildTraceSummary([])).toBeNull();
  });

  it("includes failed calls in the summary", () => {
    const summary = buildTraceSummary([
      {
        toolName: "lookup_order",
        input: { order_id: "ORD-123" },
        output: { id: "ORD-123" },
        durationMs: 0,
        retries: 0,
      },
      {
        toolName: "send_email",
        input: { to: "alice@example.com" },
        error: "SMTP unavailable",
        durationMs: 0,
        retries: 0,
      },
    ]);

    expect(summary).toContain('lookup_order OK -> {"id":"ORD-123"}');
    expect(summary).toContain("send_email FAILED: SMTP unavailable");
  });
});

describe("adapter error detection", () => {
  function createErrorEngine(): Montygate {
    const engine = {
      getToolCatalog: vi.fn(() => ""),
      getExecuteToolDescription: vi.fn(() => ""),
      getSearchToolDescription: vi.fn(() => ""),
      getExecuteToolInputSchema: vi.fn(() => ({
        type: "object",
        properties: {
          code: { type: "string", description: "Python script to execute" },
          inputs: {
            type: "object",
            description: "Variables to inject into the script",
          },
        },
        required: ["code"],
      })),
      getSearchToolInputSchema: vi.fn(() => ({
        type: "object",
        properties: {
          query: { type: "string", description: "Search query" },
          top_k: {
            type: "number",
            description: "Maximum number of results",
          },
        },
        required: ["query"],
      })),
      execute: vi.fn(async () => ({
        output: {
          status: "error",
          error: "NameError: name 'order' is not defined",
        },
        stdout: "",
        stderr: "NameError: name 'order' is not defined",
        trace: [],
        stats: {
          totalDurationMs: 0,
          montyExecutionMs: 0,
          externalCalls: 0,
          memoryPeakBytes: 0,
          stepsExecuted: 0,
        },
      })),
      search: vi.fn(() => []),
      toolCount: 0,
      getTools: vi.fn(() => []),
      getTraces: vi.fn(() => []),
      clearTraces: vi.fn(),
    };
    engine.handleToolCall = Montygate.prototype.handleToolCall.bind(engine);
    engine.vercelai = Montygate.prototype.vercelai.bind(engine);
    return engine as unknown as Montygate;
  }

  it("handleToolCall throws on sandbox error (object args)", async () => {
    const engine = createErrorEngine();
    await expect(
      engine.handleToolCall("execute", { code: "order" }),
    ).rejects.toThrow("NameError");
  });

  it("handleToolCall throws on sandbox error (JSON string args)", async () => {
    const engine = createErrorEngine();
    await expect(
      engine.handleToolCall("execute", JSON.stringify({ code: "order" })),
    ).rejects.toThrow("NameError");
  });

  it("vercelai execute throws on sandbox error", async () => {
    const engine = createErrorEngine();
    const tools = engine.vercelai();
    await expect(tools.execute.execute({ code: "order" })).rejects.toThrow(
      "NameError",
    );
  });
});
