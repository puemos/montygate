import { describe, it, expect, vi, beforeEach } from "vitest";
import type { Montygate } from "../engine.js";
import { toAnthropic, handleAnthropicToolCall } from "./anthropic.js";
import { toOpenAI, handleOpenAIToolCall } from "./openai.js";
import { toVercelAI } from "./vercel-ai.js";

function createMockEngine(): Montygate {
  return {
    getToolCatalog: vi.fn(
      () => "- lookup_order(order_id: string) - Look up an order\n",
    ),
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
  } as unknown as Montygate;
}

describe("toAnthropic", () => {
  let engine: Montygate;

  beforeEach(() => {
    engine = createMockEngine();
  });

  it("returns execute and search tools", () => {
    const tools = toAnthropic(engine);
    expect(tools).toHaveLength(2);
    expect(tools[0].name).toBe("execute");
    expect(tools[1].name).toBe("search");
  });

  it("execute tool includes tool catalog in description", () => {
    const tools = toAnthropic(engine);
    expect(tools[0].description).toContain("lookup_order");
  });

  it("execute tool has correct input_schema", () => {
    const tools = toAnthropic(engine);
    expect(tools[0].input_schema.type).toBe("object");
    expect(tools[0].input_schema.properties).toHaveProperty("code");
    expect(tools[0].input_schema.required).toContain("code");
  });

  it("search tool has correct input_schema", () => {
    const tools = toAnthropic(engine);
    expect(tools[1].input_schema.properties).toHaveProperty("query");
    expect(tools[1].input_schema.required).toContain("query");
  });
});

describe("handleAnthropicToolCall", () => {
  let engine: Montygate;

  beforeEach(() => {
    engine = createMockEngine();
  });

  it("dispatches execute call", async () => {
    const result = await handleAnthropicToolCall(engine, "execute", {
      code: "tool('lookup_order', order_id='123')",
    });
    expect(result).toEqual({ id: "123", status: "shipped" });
    expect(engine.execute).toHaveBeenCalledWith(
      "tool('lookup_order', order_id='123')",
      undefined,
    );
  });

  it("dispatches search call", async () => {
    const result = await handleAnthropicToolCall(engine, "search", {
      query: "order",
    });
    expect(result).toHaveLength(1);
    expect(engine.search).toHaveBeenCalledWith("order", undefined);
  });

  it("throws for unknown tool", async () => {
    await expect(
      handleAnthropicToolCall(engine, "unknown", {}),
    ).rejects.toThrow("Unknown tool: unknown");
  });
});

describe("toOpenAI", () => {
  let engine: Montygate;

  beforeEach(() => {
    engine = createMockEngine();
  });

  it("returns function type tools", () => {
    const tools = toOpenAI(engine);
    expect(tools).toHaveLength(2);
    expect(tools[0].type).toBe("function");
    expect(tools[0].function.name).toBe("execute");
    expect(tools[1].type).toBe("function");
    expect(tools[1].function.name).toBe("search");
  });

  it("execute tool has parameters", () => {
    const tools = toOpenAI(engine);
    expect(tools[0].function.parameters.type).toBe("object");
    expect(tools[0].function.parameters.properties).toHaveProperty("code");
  });
});

describe("handleOpenAIToolCall", () => {
  let engine: Montygate;

  beforeEach(() => {
    engine = createMockEngine();
  });

  it("dispatches execute and returns JSON string", async () => {
    const result = await handleOpenAIToolCall(
      engine,
      "execute",
      JSON.stringify({ code: "42" }),
    );
    expect(JSON.parse(result)).toEqual({ id: "123", status: "shipped" });
  });

  it("dispatches search and returns JSON string", async () => {
    const result = await handleOpenAIToolCall(
      engine,
      "search",
      JSON.stringify({ query: "order" }),
    );
    const parsed = JSON.parse(result);
    expect(parsed).toHaveLength(1);
    expect(parsed[0].name).toBe("lookup_order");
  });
});

describe("toVercelAI", () => {
  let engine: Montygate;

  beforeEach(() => {
    engine = createMockEngine();
  });

  it("returns execute and search tools", () => {
    const tools = toVercelAI(engine);
    expect(tools).toHaveProperty("execute");
    expect(tools).toHaveProperty("search");
  });

  it("execute tool has description and parameters", () => {
    const tools = toVercelAI(engine);
    expect(tools.execute.description).toContain("lookup_order");
    expect(tools.execute.parameters).toBeDefined();
  });

  it("execute tool dispatches correctly", async () => {
    const tools = toVercelAI(engine);
    const result = await tools.execute.execute({ code: "42" });
    expect(result).toEqual({ id: "123", status: "shipped" });
  });

  it("search tool dispatches correctly", async () => {
    const tools = toVercelAI(engine);
    const result = await tools.search.execute({ query: "order" });
    expect(result).toHaveLength(1);
  });
});
