import type { z } from "zod";
import { zodToJsonSchema } from "./schema.js";
import type {
  MontygateConfig,
  ToolOptions,
  ToolHandle,
  ExecutionResult,
  SearchResult,
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
      description: string | null;
      inputSchema: unknown;
    },
    run: (args: unknown) => Promise<unknown>,
  ): void;
  execute(
    code: string,
    inputs?: Record<string, unknown> | null,
  ): Promise<NativeExecutionResult>;
  search(query: string, topK?: number | null): NativeSearchResult[];
  getToolCatalog(): string;
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

// Try to load the native binding (platform-specific .node file)
let NativeEngineClass: NativeEngineConstructor;
try {
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const binding = require("../montygate.node");
  NativeEngineClass = binding.NativeEngine;
} catch {
  // Try platform-specific binary name: montygate.{platform}-{arch}.node
  try {
    const platform = process.platform;
    const arch = process.arch;
    const platformMap: Record<string, string> = {
      darwin: "darwin",
      linux: "linux",
      win32: "win32",
    };
    const archMap: Record<string, string> = {
      x64: "x64",
      arm64: "arm64",
    };
    const p = platformMap[platform] ?? platform;
    const a = archMap[arch] ?? arch;
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const binding = require(`../montygate.${p}-${a}.node`);
    NativeEngineClass = binding.NativeEngine;
  } catch {
    // Native binding not available — will throw at construction time
    NativeEngineClass = undefined as unknown as NativeEngineConstructor;
  }
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
  private tools = new Map<string, ToolHandle>();

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
  }

  /**
   * Register a tool with a Zod schema and async handler.
   */
  tool<T extends z.ZodType>(name: string, options: ToolOptions<T>): this {
    const inputSchema = zodToJsonSchema(options.params);

    const handle: ToolHandle = {
      name,
      description: options.description,
      inputSchema,
    };
    this.tools.set(name, handle);

    this.native.registerTool(
      {
        name,
        description: options.description,
        inputSchema,
      },
      options.run as (args: unknown) => Promise<unknown>,
    );

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

  /** Number of registered tools. */
  get toolCount(): number {
    return this.native.toolCount();
  }

  /** Get all registered tool handles. */
  getTools(): ToolHandle[] {
    return Array.from(this.tools.values());
  }

  /** Get all execution traces. */
  getTraces(): TraceEntry[] {
    return this.native.getTraces().map(mapTraceEntry);
  }

  /** Clear all execution traces. */
  clearTraces(): void {
    this.native.clearTraces();
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
  };
}

function mapExecutionResult(raw: NativeExecutionResult): ExecutionResult {
  return {
    output: raw.output,
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
