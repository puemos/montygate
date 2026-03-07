export { Montygate } from "./engine.js";
export { zodToJsonSchema } from "./schema.js";

// Adapters
export { toAnthropic, handleAnthropicToolCall } from "./adapters/anthropic.js";
export type { AnthropicTool } from "./adapters/anthropic.js";

export { toOpenAI, handleOpenAIToolCall } from "./adapters/openai.js";
export type { OpenAITool } from "./adapters/openai.js";

export { toVercelAI } from "./adapters/vercel-ai.js";
export type { VercelAIToolDef } from "./adapters/vercel-ai.js";

// Types
export type {
  MontygateConfig,
  RetryConfig,
  ExecutionLimitsConfig,
  ResourceLimitsConfig,
  PolicyConfig,
  PolicyRule,
  ToolOptions,
  ToolHandle,
  ExecutionResult,
  TraceEntry,
  ExecutionStats,
  SearchResult,
} from "./types.js";
