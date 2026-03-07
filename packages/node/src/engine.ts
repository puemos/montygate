import type { z } from "zod";
import { detectFormat } from "./detect.js";
import {
  type AnyToolDefinition,
  normalizeTool,
  type ToolHandlerMap,
} from "./normalize.js";
import { zodToJsonSchema } from "./schema.js";
import type {
  ExecutionResult,
  MontygateConfig,
  SearchResult,
  ToolHandle,
  ToolOptions,
  TraceEntry,
} from "./types.js";

/**
 * Binding types from the native NAPI module.
 * These match the #[napi(object)] structs in montygate-napi.
 */
interface NativeEngine {
  registerTool(
    definition: {
      name: string;
      description?: string;
      inputSchema: unknown;
      outputSchema?: unknown;
    },
    run: (args: unknown) => Promise<unknown>,
  ): void;
  execute(
    code: string,
    inputs?: Record<string, unknown> | null,
  ): Promise<NativeExecutionResult>;
  search(query: string, topK?: number | null): NativeSearchResult[];
  getToolCatalog(): string;
  getExecuteToolDescription(): string;
  getSearchToolDescription(): string;
  getSystemPrompt(): string;
  getExecuteToolInputSchema(): Record<string, unknown>;
  getSearchToolInputSchema(): Record<string, unknown>;
  toolCount(): number;
  getTraces(): NativeTraceEntry[];
  clearTraces(): void;
}

interface NativeExecutionResult {
  output: unknown;
  stdout: string;
  stderr: string;
  trace: NativeTraceEntry[];
  stats: {
    totalDurationMs: number;
    montyExecutionMs: number;
    externalCalls: number;
    memoryPeakBytes: number;
    stepsExecuted: number;
  };
}

interface NativeTraceEntry {
  toolName: string;
  input: unknown;
  output?: unknown;
  error?: string;
  durationMs: number;
  retries: number;
}

interface NativeSearchResult {
  name: string;
  description: string | null;
  inputSchema: unknown;
  outputSchema?: unknown;
}

interface NativeEngineConstructor {
  new (config?: {
    retry?: { maxRetries?: number; baseDelayMs?: number } | null;
    limits?: { timeoutMs?: number; maxConcurrent?: number } | null;
    resourceLimits?: {
      maxExecutionTimeMs?: number;
      maxMemoryBytes?: number;
      maxStackDepth?: number;
      maxExternalCalls?: number;
      maxCodeLength?: number;
    } | null;
    policy?: {
      defaultAction?: string | null;
      rules?:
        | {
            matchPattern: string;
            action: string;
            rateLimit?: string | null;
          }[]
        | null;
    } | null;
  }): NativeEngine;
}

// Maps Node.js platform+arch to the NAPI-RS triple suffix used in both:
//   - published npm package names: montygate-<suffix>
//   - local .node filenames:       montygate.<suffix>.node
const triples: Record<string, string> = {
  "darwin-arm64": "darwin-arm64",
  "darwin-x64": "darwin-x64",
  "linux-x64": "linux-x64-gnu",
  "linux-arm64": "linux-arm64-gnu",
  "win32-x64": "win32-x64-msvc",
};

// Try to load the native binding.
// 1. Published: platform-specific package from optionalDependencies
// 2. Dev: local .node file built by `napi build --platform`
let NativeEngineClass: NativeEngineConstructor;
try {
  const triple = triples[`${process.platform}-${process.arch}`];
  if (!triple)
    throw new Error(
      `Unsupported platform: ${process.platform}-${process.arch}`,
    );

  let binding: { NativeEngine: NativeEngineConstructor };
  try {
    // Published: montygate-<triple> from npm
    binding = require(`montygate-${triple}`);
  } catch {
    // Dev: local .node file
    binding = require(`../montygate.${triple}.node`);
  }
  NativeEngineClass = binding.NativeEngine;
} catch {
  // Native binding not available — will throw at construction time.
  NativeEngineClass = undefined as unknown as NativeEngineConstructor;
}

/**
 * Montygate — register tools once, execute multi-tool scripts, return one result.
 *
 * ```ts
 * const gate = new Montygate();
 * gate.tool("lookup_order", {
 *   description: "Look up order by ID",
 *   params: z.object({ orderId: z.string() }),
 *   run: async ({ orderId }) => db.orders.find(orderId),
 * });
 *
 * const result = await gate.execute(`
 *   order = tool('lookup_order', order_id='123')
 *   order
 * `);
 * ```
 */
export class Montygate {
  private native: NativeEngine;
  private toolHandles = new Map<string, ToolHandle>();

  constructor(config?: MontygateConfig) {
    if (!NativeEngineClass) {
      throw new Error(
        "Native binding not found. Run `napi build --platform` in the montygate package first.",
      );
    }

    const nativeConfig: Record<string, unknown> = {};

    if (config?.retry) {
      nativeConfig.retry = {
        maxRetries: config.retry.maxRetries,
        baseDelayMs: config.retry.baseDelayMs,
      };
    }
    if (config?.limits) {
      nativeConfig.limits = {
        timeoutMs: config.limits.timeoutMs,
        maxConcurrent: config.limits.maxConcurrent,
      };
    }
    if (config?.resourceLimits) {
      nativeConfig.resourceLimits = {
        maxExecutionTimeMs: config.resourceLimits.maxExecutionTimeMs,
        maxMemoryBytes: config.resourceLimits.maxMemoryBytes,
        maxStackDepth: config.resourceLimits.maxStackDepth,
        maxExternalCalls: config.resourceLimits.maxExternalCalls,
        maxCodeLength: config.resourceLimits.maxCodeLength,
      };
    }
    if (config?.policy) {
      nativeConfig.policy = {
        defaultAction: config.policy.defaultAction,
        rules: config.policy.rules?.map((r) => ({
          matchPattern: r.matchPattern,
          action: r.action,
          rateLimit: r.rateLimit,
        })),
      };
    }

    this.native = new NativeEngineClass(
      Object.keys(nativeConfig).length > 0 ? nativeConfig : undefined,
    ) as NativeEngine;

    if (config?.tools) {
      this.tools(config.tools, config.handlers);
    }
  }

  /**
   * Register a tool with a Zod schema and async handler.
   */
  tool<T extends z.ZodType>(name: string, options: ToolOptions<T>): this {
    const inputSchema = zodToJsonSchema(options.params);
    const outputSchema = options.returns
      ? zodToJsonSchema(options.returns)
      : undefined;

    const handle: ToolHandle = {
      name,
      description: options.description,
      inputSchema,
      outputSchema,
    };
    this.toolHandles.set(name, handle);

    this.native.registerTool(
      {
        name,
        description: options.description,
        inputSchema,
        outputSchema,
      },
      options.run as (args: unknown) => Promise<unknown>,
    );

    return this;
  }

  /**
   * Register tools from any supported format (OpenAI, Anthropic, Vercel AI, etc.).
   * Auto-detects the format and normalizes each tool.
   *
   * @param defs - Array of tool definitions or object keyed by tool name (Vercel AI style).
   * @param handlers - Optional map of tool name → async handler for formats without embedded handlers.
   */
  tools(
    defs: AnyToolDefinition[] | Record<string, AnyToolDefinition>,
    handlers?: ToolHandlerMap,
  ): this {
    const entries: Array<{ tool: AnyToolDefinition; keyName?: string }> =
      Array.isArray(defs)
        ? defs.map((t) => ({ tool: t }))
        : Object.entries(defs).map(([key, t]) => ({ tool: t, keyName: key }));

    for (const { tool: rawTool, keyName } of entries) {
      const format = keyName ? "vercel-ai" : detectFormat(rawTool);

      if (format === "unknown") {
        throw new Error(
          `Could not detect tool format. Ensure the tool matches a supported shape (OpenAI, Anthropic, Vercel AI, or OpenAI Agents SDK).`,
        );
      }

      const normalized = normalizeTool(rawTool, format, handlers, keyName);

      if (!normalized.handler && !handlers?.[normalized.name]) {
        throw new Error(
          `Tool '${normalized.name}' (detected as ${format} format) has no handler. Pass one in the handlers map.`,
        );
      }

      // Safe: the guard above ensures at least one is defined.
      const handler = (normalized.handler ?? handlers?.[normalized.name]) as (
        args: unknown,
      ) => Promise<unknown>;

      const handle: ToolHandle = {
        name: normalized.name,
        description: normalized.description,
        inputSchema: normalized.inputSchema,
      };
      this.toolHandles.set(normalized.name, handle);

      this.native.registerTool(
        {
          name: normalized.name,
          description: normalized.description,
          inputSchema: normalized.inputSchema,
        },
        handler,
      );
    }

    return this;
  }

  /**
   * Execute a Python script with access to all registered tools.
   * Only the final expression value is returned.
   */
  async execute(
    code: string,
    inputs?: Record<string, unknown>,
  ): Promise<ExecutionResult> {
    const raw = await this.native.execute(code, inputs ?? null);
    return mapExecutionResult(raw);
  }

  /**
   * Search registered tools by keyword.
   */
  search(query: string, topK?: number): SearchResult[] {
    return this.native.search(query, topK).map(mapSearchResult);
  }

  /**
   * Get a formatted catalog of all registered tools.
   * Useful for including in LLM prompts.
   */
  getToolCatalog(): string {
    return this.native.getToolCatalog();
  }

  /**
   * Get the canonical "execute" tool description for LLM adapters.
   * Includes the tool catalog, usage instructions, and examples.
   */
  getExecuteToolDescription(): string {
    return this.native.getExecuteToolDescription();
  }

  /**
   * Get the canonical "search" tool description for LLM adapters.
   */
  getSearchToolDescription(): string {
    return this.native.getSearchToolDescription();
  }

  /** Number of registered tools. */
  get toolCount(): number {
    return this.native.toolCount();
  }

  /** Get all registered tool handles. */
  getTools(): ToolHandle[] {
    return Array.from(this.toolHandles.values());
  }

  /** Get all execution traces. */
  getTraces(): TraceEntry[] {
    return this.native.getTraces().map(mapTraceEntry);
  }

  /** Clear all execution traces. */
  clearTraces(): void {
    this.native.clearTraces();
  }

  /**
   * Get a system prompt that guides the LLM toward efficient single-script usage.
   * Include this in your API call's system message for best results.
   */
  getSystemPrompt(): string {
    return this.native.getSystemPrompt();
  }

  /**
   * Get the canonical JSON Schema for the `execute` tool's input parameters.
   * Use this in adapters instead of hard-coding the schema.
   */
  getExecuteToolInputSchema(): Record<string, unknown> {
    return this.native.getExecuteToolInputSchema() as Record<string, unknown>;
  }

  /**
   * Get the canonical JSON Schema for the `search` tool's input parameters.
   * Use this in adapters instead of hard-coding the schema.
   */
  getSearchToolInputSchema(): Record<string, unknown> {
    return this.native.getSearchToolInputSchema() as Record<string, unknown>;
  }
}

function mapTraceEntry(entry: NativeTraceEntry): TraceEntry {
  return {
    toolName: entry.toolName,
    input: entry.input,
    output: entry.output,
    error: entry.error,
    durationMs: entry.durationMs,
    retries: entry.retries,
  };
}

function mapSearchResult(result: NativeSearchResult): SearchResult {
  return {
    name: result.name,
    description: result.description ?? undefined,
    inputSchema: (result.inputSchema ?? {}) as Record<string, unknown>,
    outputSchema: result.outputSchema
      ? (result.outputSchema as Record<string, unknown>)
      : undefined,
  };
}

function mapExecutionResult(raw: NativeExecutionResult): ExecutionResult {
  // If the script returned null/None but printed something, surface stdout
  // so the LLM gets feedback instead of a bare null.
  let output = raw.output;
  if (output == null && raw.stdout.length > 0) {
    output = { result: null, stdout: raw.stdout };
  }

  return {
    output,
    stdout: raw.stdout,
    stderr: raw.stderr,
    trace: raw.trace.map(mapTraceEntry),
    stats: {
      totalDurationMs: raw.stats.totalDurationMs,
      montyExecutionMs: raw.stats.montyExecutionMs,
      externalCalls: raw.stats.externalCalls,
      memoryPeakBytes: raw.stats.memoryPeakBytes,
      stepsExecuted: raw.stats.stepsExecuted,
    },
  };
}
