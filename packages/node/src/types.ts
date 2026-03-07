import type { z } from "zod";
import type { AnyToolDefinition, ToolHandlerMap } from "./normalize.js";

/** Configuration for the Montygate engine. */
export interface MontygateConfig {
  retry?: RetryConfig;
  limits?: ExecutionLimitsConfig;
  resourceLimits?: ResourceLimitsConfig;
  policy?: PolicyConfig;
  /** Tool definitions in any supported format (OpenAI, Anthropic, Vercel AI, etc.). */
  tools?: AnyToolDefinition[] | Record<string, AnyToolDefinition>;
  /** Handler map for tool formats without embedded handlers. */
  handlers?: ToolHandlerMap;
}

export interface RetryConfig {
  maxRetries?: number;
  baseDelayMs?: number;
}

export interface ExecutionLimitsConfig {
  timeoutMs?: number;
  maxConcurrent?: number;
}

export interface ResourceLimitsConfig {
  maxExecutionTimeMs?: number;
  maxMemoryBytes?: number;
  maxStackDepth?: number;
  maxExternalCalls?: number;
  maxCodeLength?: number;
}

export interface PolicyConfig {
  defaultAction?: "allow" | "deny" | "require_approval";
  rules?: PolicyRule[];
}

export interface PolicyRule {
  matchPattern: string;
  action: "allow" | "deny" | "require_approval";
  rateLimit?: string;
}

/** Options passed when registering a tool. */
export interface ToolOptions<T extends z.ZodType = z.ZodType> {
  description?: string;
  params: T;
  /** Zod schema describing the tool's return type (shown in catalog so the LLM knows field names). */
  returns?: z.ZodType;
  run: (input: z.infer<T>) => Promise<unknown>;
}

/** A registered tool handle. */
export interface ToolHandle {
  name: string;
  description?: string;
  inputSchema: Record<string, unknown>;
  outputSchema?: Record<string, unknown>;
}

/** Result of executing a script. */
export interface ExecutionResult {
  output: unknown;
  stdout: string;
  stderr: string;
  trace: TraceEntry[];
  stats: ExecutionStats;
}

export interface TraceEntry {
  toolName: string;
  input: unknown;
  output?: unknown;
  error?: string;
  durationMs: number;
  retries: number;
}

export interface ExecutionStats {
  totalDurationMs: number;
  montyExecutionMs: number;
  externalCalls: number;
  memoryPeakBytes: number;
  stepsExecuted: number;
}

/** Search result from tool discovery. */
export interface SearchResult {
  name: string;
  description?: string;
  inputSchema: Record<string, unknown>;
  outputSchema?: Record<string, unknown>;
}
