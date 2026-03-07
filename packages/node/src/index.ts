export { Montygate } from "./engine.js";
export type { AnthropicTool, OpenAITool, VercelAIToolDef } from "./engine.js";
export { zodToJsonSchema } from "./schema.js";

// Detection & normalization
export type { ToolFormat } from "./detect.js";
export { detectFormat, isZodSchema } from "./detect.js";
export type { AnyToolDefinition, NormalizedTool, ToolHandlerMap } from "./normalize.js";
export { normalizeTool } from "./normalize.js";

// Adapter utilities (for advanced use)
export { unwrapExecutionResult, buildTraceSummary } from "./adapters/utils.js";

// Types
export type {
  ExecutionLimitsConfig,
  ExecutionResult,
  ExecutionStats,
  MontygateConfig,
  PolicyConfig,
  PolicyRule,
  ResourceLimitsConfig,
  RetryConfig,
  SearchResult,
  ToolHandle,
  ToolOptions,
  TraceEntry,
} from "./types.js";
