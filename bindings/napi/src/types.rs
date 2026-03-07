use napi_derive::napi;

/// Configuration for the NativeEngine, passed from JS.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct NapiConfig {
    /// Retry configuration
    pub retry: Option<NapiRetryConfig>,
    /// Execution limits (concurrency, timeout per tool call)
    pub limits: Option<NapiExecutionLimits>,
    /// Resource limits for the Monty sandbox
    pub resource_limits: Option<NapiResourceLimits>,
    /// Policy configuration
    pub policy: Option<NapiPolicyConfig>,
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct NapiRetryConfig {
    pub max_retries: Option<u32>,
    pub base_delay_ms: Option<u32>,
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct NapiExecutionLimits {
    pub timeout_ms: Option<u32>,
    pub max_concurrent: Option<u32>,
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct NapiResourceLimits {
    pub max_execution_time_ms: Option<u32>,
    pub max_memory_bytes: Option<u32>,
    pub max_stack_depth: Option<u32>,
    pub max_external_calls: Option<u32>,
    pub max_code_length: Option<u32>,
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct NapiPolicyConfig {
    pub default_action: Option<String>,
    pub rules: Option<Vec<NapiPolicyRule>>,
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct NapiPolicyRule {
    pub match_pattern: String,
    pub action: String,
    pub rate_limit: Option<String>,
}

/// Tool definition passed from JS when registering a tool.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct NapiToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// Result of executing a script, returned to JS.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct NapiExecutionResult {
    pub output: serde_json::Value,
    pub stdout: String,
    pub stderr: String,
    pub trace: Vec<NapiTraceEntry>,
    pub stats: NapiExecutionStats,
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct NapiTraceEntry {
    pub tool_name: String,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u32,
    pub retries: u32,
}

#[napi(object)]
#[derive(Debug, Clone)]
pub struct NapiExecutionStats {
    pub total_duration_ms: u32,
    pub monty_execution_ms: u32,
    pub external_calls: u32,
    pub memory_peak_bytes: u32,
    pub steps_executed: u32,
}

/// Search result returned to JS.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct NapiSearchResult {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

// --- Conversions from core types ---

impl From<&montygate_core::types::ToolCall> for NapiTraceEntry {
    fn from(call: &montygate_core::types::ToolCall) -> Self {
        Self {
            tool_name: call.tool.clone(),
            input: call.arguments.clone(),
            output: call.result.clone(),
            error: call.error.clone(),
            duration_ms: call.duration_ms as u32,
            retries: call.retries,
        }
    }
}

impl From<&montygate_core::types::ExecutionStats> for NapiExecutionStats {
    fn from(stats: &montygate_core::types::ExecutionStats) -> Self {
        Self {
            total_duration_ms: stats.total_duration_ms as u32,
            monty_execution_ms: stats.monty_execution_ms as u32,
            external_calls: stats.external_calls as u32,
            memory_peak_bytes: stats.memory_peak_bytes as u32,
            steps_executed: stats.steps_executed as u32,
        }
    }
}

impl From<&montygate_core::types::ExecutionResult> for NapiExecutionResult {
    fn from(result: &montygate_core::types::ExecutionResult) -> Self {
        Self {
            output: result.output.clone(),
            stdout: result.stdout.clone(),
            stderr: result.stderr.clone(),
            trace: result.trace.iter().map(NapiTraceEntry::from).collect(),
            stats: NapiExecutionStats::from(&result.stats),
        }
    }
}

impl From<&montygate_core::types::ToolDefinition> for NapiSearchResult {
    fn from(def: &montygate_core::types::ToolDefinition) -> Self {
        Self {
            name: def.name.clone(),
            description: def.description.clone(),
            input_schema: def.input_schema.clone(),
        }
    }
}

impl From<&NapiToolDefinition> for montygate_core::types::ToolDefinition {
    fn from(def: &NapiToolDefinition) -> Self {
        Self {
            name: def.name.clone(),
            description: def.description.clone(),
            input_schema: def.input_schema.clone(),
        }
    }
}

// --- Config conversions ---

impl NapiRetryConfig {
    pub fn to_core(&self) -> montygate_core::types::RetryConfig {
        montygate_core::types::RetryConfig {
            max_retries: self.max_retries.unwrap_or(3),
            base_delay_ms: self.base_delay_ms.unwrap_or(100) as u64,
        }
    }
}

impl NapiExecutionLimits {
    pub fn to_core(&self) -> montygate_core::types::ExecutionLimits {
        montygate_core::types::ExecutionLimits {
            timeout_ms: self.timeout_ms.unwrap_or(30_000) as u64,
            max_concurrent: self.max_concurrent.unwrap_or(5) as usize,
        }
    }
}

impl NapiResourceLimits {
    pub fn to_core(&self) -> montygate_core::types::ResourceLimits {
        montygate_core::types::ResourceLimits {
            max_execution_time_ms: self.max_execution_time_ms.unwrap_or(30_000) as u64,
            max_memory_bytes: self.max_memory_bytes.unwrap_or(50 * 1024 * 1024) as usize,
            max_stack_depth: self.max_stack_depth.unwrap_or(100) as usize,
            max_external_calls: self.max_external_calls.unwrap_or(50) as usize,
            max_code_length: self.max_code_length.unwrap_or(10_000) as usize,
        }
    }
}
