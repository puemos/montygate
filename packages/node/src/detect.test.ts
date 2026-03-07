import { describe, expect, it } from "vitest";
import { z } from "zod";
import { detectFormat, isZodSchema } from "./detect.js";
import { normalizeTool } from "./normalize.js";

// === detectFormat ===

describe("detectFormat", () => {
  it("detects openai-chat format", () => {
    const tool = {
      type: "function",
      function: {
        name: "get_weather",
        description: "Get weather",
        parameters: {
          type: "object",
          properties: { city: { type: "string" } },
        },
      },
    };
    expect(detectFormat(tool)).toBe("openai-chat");
  });

  it("detects openai-responses format", () => {
    const tool = {
      type: "function",
      name: "get_weather",
      description: "Get weather",
      parameters: { type: "object", properties: { city: { type: "string" } } },
    };
    expect(detectFormat(tool)).toBe("openai-responses");
  });

  it("detects anthropic-raw format", () => {
    const tool = {
      name: "get_weather",
      description: "Get weather",
      input_schema: {
        type: "object",
        properties: { city: { type: "string" } },
      },
    };
    expect(detectFormat(tool)).toBe("anthropic-raw");
  });

  it("detects anthropic-zod format", () => {
    const tool = {
      name: "get_weather",
      description: "Get weather",
      inputSchema: z.object({ city: z.string() }),
      run: async () => ({}),
    };
    expect(detectFormat(tool)).toBe("anthropic-zod");
  });

  it("detects openai-agents format", () => {
    const tool = {
      name: "get_weather",
      description: "Get weather",
      parameters: z.object({ city: z.string() }),
      execute: async () => ({}),
    };
    expect(detectFormat(tool)).toBe("openai-agents");
  });

  it("detects vercel-ai format", () => {
    const tool = {
      description: "Get weather",
      parameters: z.object({ city: z.string() }),
      execute: async () => ({}),
    };
    expect(detectFormat(tool)).toBe("vercel-ai");
  });

  it("returns unknown for unrecognized shape", () => {
    expect(detectFormat({ foo: "bar" })).toBe("unknown");
    expect(detectFormat(null)).toBe("unknown");
    expect(detectFormat(42)).toBe("unknown");
    expect(detectFormat("string")).toBe("unknown");
  });

  it("prioritizes openai-chat over openai-responses when function object present", () => {
    // Has both .function.name AND could look like responses, but function object takes priority
    const tool = {
      type: "function",
      function: { name: "test", parameters: {} },
      name: "test", // extra field shouldn't confuse it
    };
    expect(detectFormat(tool)).toBe("openai-chat");
  });

  it("prioritizes anthropic-zod over anthropic-raw when Zod inputSchema present", () => {
    const tool = {
      name: "test",
      inputSchema: z.object({ x: z.string() }),
      input_schema: { type: "object" }, // also has raw field
      run: async () => ({}),
    };
    expect(detectFormat(tool)).toBe("anthropic-zod");
  });

  it("prioritizes openai-agents over openai-responses when execute present", () => {
    const tool = {
      type: "function",
      name: "test",
      parameters: { type: "object" },
      execute: async () => ({}),
    };
    // Has type: "function" + name + parameters + execute
    // openai-agents checks name + parameters + execute first
    expect(detectFormat(tool)).toBe("openai-agents");
  });
});

// === isZodSchema ===

describe("isZodSchema", () => {
  it("returns true for z.object()", () => {
    expect(isZodSchema(z.object({ x: z.string() }))).toBe(true);
  });

  it("returns true for z.string()", () => {
    expect(isZodSchema(z.string())).toBe(true);
  });

  it("returns true for z.number()", () => {
    expect(isZodSchema(z.number())).toBe(true);
  });

  it("returns true for z.array()", () => {
    expect(isZodSchema(z.array(z.string()))).toBe(true);
  });

  it("returns false for plain JSON Schema object", () => {
    expect(isZodSchema({ type: "object", properties: {} })).toBe(false);
  });

  it("returns false for null/undefined", () => {
    expect(isZodSchema(null)).toBe(false);
    expect(isZodSchema(undefined)).toBe(false);
  });

  it("returns false for primitive values", () => {
    expect(isZodSchema("string")).toBe(false);
    expect(isZodSchema(42)).toBe(false);
  });
});

// === normalizeTool ===

describe("normalizeTool", () => {
  const handler = async () => ({ result: "ok" });

  it("normalizes openai-chat format", () => {
    const tool = {
      type: "function",
      function: {
        name: "get_weather",
        description: "Get weather",
        parameters: {
          type: "object",
          properties: { city: { type: "string" } },
        },
      },
    };
    const result = normalizeTool(tool, "openai-chat", { get_weather: handler });
    expect(result.name).toBe("get_weather");
    expect(result.description).toBe("Get weather");
    expect(result.inputSchema).toEqual({
      type: "object",
      properties: { city: { type: "string" } },
    });
    expect(result.handler).toBe(handler);
  });

  it("normalizes openai-responses format", () => {
    const tool = {
      type: "function",
      name: "search",
      description: "Search the web",
      parameters: { type: "object", properties: { query: { type: "string" } } },
    };
    const result = normalizeTool(tool, "openai-responses", { search: handler });
    expect(result.name).toBe("search");
    expect(result.description).toBe("Search the web");
    expect(result.inputSchema.properties).toBeDefined();
    expect(result.handler).toBe(handler);
  });

  it("normalizes anthropic-raw format", () => {
    const tool = {
      name: "get_weather",
      description: "Get weather",
      input_schema: {
        type: "object",
        properties: { city: { type: "string" } },
      },
    };
    const result = normalizeTool(tool, "anthropic-raw", {
      get_weather: handler,
    });
    expect(result.name).toBe("get_weather");
    expect(result.description).toBe("Get weather");
    expect(result.inputSchema).toEqual({
      type: "object",
      properties: { city: { type: "string" } },
    });
    expect(result.handler).toBe(handler);
  });

  it("normalizes anthropic-zod format with Zod → JSON Schema conversion", () => {
    const run = async () => ({ temp: 72 });
    const tool = {
      name: "get_weather",
      description: "Get weather",
      inputSchema: z.object({ city: z.string() }),
      run,
    };
    const result = normalizeTool(tool, "anthropic-zod");
    expect(result.name).toBe("get_weather");
    expect(result.inputSchema.type).toBe("object");
    expect(
      (result.inputSchema.properties as Record<string, unknown>)?.city,
    ).toBeDefined();
    expect(result.handler).toBe(run);
  });

  it("normalizes openai-agents format with Zod → JSON Schema conversion", () => {
    const execute = async () => ({ done: true });
    const tool = {
      name: "run_task",
      description: "Run a task",
      parameters: z.object({ task_id: z.string() }),
      execute,
    };
    const result = normalizeTool(tool, "openai-agents");
    expect(result.name).toBe("run_task");
    expect(result.inputSchema.type).toBe("object");
    expect(result.handler).toBe(execute);
  });

  it("normalizes vercel-ai format with key as name", () => {
    const execute = async () => ({ data: "ok" });
    const tool = {
      description: "Get weather",
      parameters: z.object({ city: z.string() }),
      execute,
    };
    const result = normalizeTool(tool, "vercel-ai", undefined, "weather");
    expect(result.name).toBe("weather");
    expect(result.description).toBe("Get weather");
    expect(result.inputSchema.type).toBe("object");
    expect(result.handler).toBe(execute);
  });

  it("throws for unknown format", () => {
    expect(() => normalizeTool({}, "unknown" as any)).toThrow(
      "Unknown tool format",
    );
  });

  it("returns undefined handler when no handler provided for raw format", () => {
    const tool = {
      type: "function",
      function: {
        name: "no_handler",
        parameters: { type: "object" },
      },
    };
    const result = normalizeTool(tool, "openai-chat");
    expect(result.handler).toBeUndefined();
  });
});
