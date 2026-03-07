use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Unique identifier for a tool call in the execution trace
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolCallId(pub String);

impl ToolCallId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for ToolCallId {
    fn default() -> Self {
        Self::new()
    }
}

/// Represents a call to an external tool during execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    pub tool: String,
    pub arguments: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub retries: u32,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ToolCall {
    pub fn new(tool: String, arguments: serde_json::Value) -> Self {
        Self {
            id: ToolCallId::new(),
            tool,
            arguments,
            result: None,
            error: None,
            duration_ms: 0,
            retries: 0,
            timestamp: chrono::Utc::now(),
        }
    }

    pub fn with_result(mut self, result: serde_json::Value, duration_ms: u64) -> Self {
        self.result = Some(result);
        self.duration_ms = duration_ms;
        self
    }

    pub fn with_error(mut self, error: String, duration_ms: u64) -> Self {
        self.error = Some(error);
        self.duration_ms = duration_ms;
        self
    }

    pub fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }
}

/// Statistics about code execution
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionStats {
    pub total_duration_ms: u64,
    pub monty_execution_ms: u64,
    pub external_calls: usize,
    pub memory_peak_bytes: usize,
    pub steps_executed: usize,
}

/// Complete result of executing a Monty program
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub output: serde_json::Value,
    pub stdout: String,
    pub stderr: String,
    pub trace: Vec<ToolCall>,
    pub stats: ExecutionStats,
}

impl ExecutionResult {
    pub fn success(output: impl Into<serde_json::Value>) -> Self {
        Self {
            output: output.into(),
            stdout: String::new(),
            stderr: String::new(),
            trace: Vec::new(),
            stats: ExecutionStats::default(),
        }
    }

    pub fn with_stdout(mut self, stdout: String) -> Self {
        self.stdout = stdout;
        self
    }

    pub fn with_stderr(mut self, stderr: String) -> Self {
        self.stderr = stderr;
        self
    }

    pub fn with_trace(mut self, trace: Vec<ToolCall>) -> Self {
        self.trace = trace;
        self
    }

    pub fn with_stats(mut self, stats: ExecutionStats) -> Self {
        self.stats = stats;
        self
    }
}

/// Resource limits for Monty execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub max_execution_time_ms: u64,
    pub max_memory_bytes: usize,
    pub max_stack_depth: usize,
    pub max_external_calls: usize,
    pub max_code_length: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_execution_time_ms: 30_000,      // 30 seconds
            max_memory_bytes: 50 * 1024 * 1024, // 50 MB
            max_stack_depth: 100,
            max_external_calls: 50,
            max_code_length: 10_000,
        }
    }
}

/// Execution limits for the scheduler (concurrency + timeout)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionLimits {
    /// Timeout per tool call in milliseconds
    pub timeout_ms: u64,
    /// Maximum number of concurrent tool calls
    pub max_concurrent: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            max_concurrent: 5,
        }
    }
}

/// Error types for Montygate
#[derive(Error, Debug)]
pub enum MontygateError {
    #[error("Execution error: {0}")]
    Execution(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Policy violation: {0}")]
    PolicyViolation(String),

    #[error("Rate limit exceeded: {0}")]
    RateLimitExceeded(String),

    #[error("Resource limit exceeded: {0}")]
    ResourceLimitExceeded(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Max retries exceeded: {0}")]
    MaxRetries(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Type check error: {0}")]
    TypeCheck(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("External call interrupted")]
    Interrupted,
}

pub type Result<T> = std::result::Result<T, MontygateError>;

/// Tool definition registered by the developer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// Retry configuration for tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of retries before giving up (default: 3)
    pub max_retries: u32,
    /// Base delay in milliseconds for exponential backoff (default: 100)
    pub base_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 100,
        }
    }
}

/// Policy action for tool access control
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Allow,
    Deny,
    RequireApproval,
}

/// Policy configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PolicyConfig {
    #[serde(default)]
    pub defaults: PolicyDefaults,
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDefaults {
    pub action: PolicyAction,
}

impl Default for PolicyDefaults {
    fn default() -> Self {
        Self {
            action: PolicyAction::Allow,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    pub match_pattern: String,
    pub action: PolicyAction,
    #[serde(default)]
    pub rate_limit: Option<String>,
}

/// Input for running a program
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunProgramInput {
    pub code: String,
    #[serde(default)]
    pub inputs: HashMap<String, serde_json::Value>,
    #[serde(default = "default_true")]
    pub type_check: bool,
}

fn default_true() -> bool {
    true
}

/// External call request from Monty execution
#[derive(Debug, Clone)]
pub struct ExternalCall {
    pub function_name: String,
    pub arguments: serde_json::Value,
}

/// State of Monty execution
#[derive(Debug, Clone)]
pub enum ExecutionState {
    Running,
    Paused(ExternalCall),
    Complete(ExecutionResult),
    Error(String),
}

/// Snapshot for resuming execution
#[derive(Debug, Clone)]
pub struct ExecutionSnapshot {
    pub id: String,
    pub state: Vec<u8>,
    pub pending_call: Option<ExternalCall>,
    pub trace: Vec<ToolCall>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // === ToolCallId ===

    #[test]
    fn test_tool_call_id_new() {
        let id1 = ToolCallId::new();
        let id2 = ToolCallId::new();
        assert_ne!(id1, id2);
        assert!(!id1.0.is_empty());
    }

    #[test]
    fn test_tool_call_id_default() {
        let id = ToolCallId::default();
        assert!(!id.0.is_empty());
    }

    #[test]
    fn test_tool_call_id_serialization() {
        let id = ToolCallId("test-id".to_string());
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: ToolCallId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }

    // === ToolCall ===

    #[test]
    fn test_tool_call_creation() {
        let call = ToolCall::new(
            "create_issue".to_string(),
            serde_json::json!({"repo": "test/repo", "title": "Test"}),
        );

        assert_eq!(call.tool, "create_issue");
        assert!(call.result.is_none());
        assert!(call.error.is_none());
        assert_eq!(call.duration_ms, 0);
        assert_eq!(call.retries, 0);
    }

    #[test]
    fn test_tool_call_with_result() {
        let call = ToolCall::new(
            "create_issue".to_string(),
            serde_json::json!({}),
        )
        .with_result(serde_json::json!({"id": 123}), 42);

        assert_eq!(call.result, Some(serde_json::json!({"id": 123})));
        assert_eq!(call.duration_ms, 42);
        assert!(call.error.is_none());
    }

    #[test]
    fn test_tool_call_with_error() {
        let call = ToolCall::new(
            "create_issue".to_string(),
            serde_json::json!({}),
        )
        .with_error("connection timeout".to_string(), 5000);

        assert!(call.result.is_none());
        assert_eq!(call.error, Some("connection timeout".to_string()));
        assert_eq!(call.duration_ms, 5000);
    }

    #[test]
    fn test_tool_call_with_retries() {
        let call = ToolCall::new("test".to_string(), serde_json::json!({}))
            .with_retries(3);
        assert_eq!(call.retries, 3);
    }

    #[test]
    fn test_tool_call_serialization() {
        let call = ToolCall::new(
            "create_issue".to_string(),
            serde_json::json!({"title": "Bug"}),
        )
        .with_result(serde_json::json!({"id": 1}), 100);

        let json = serde_json::to_string(&call).unwrap();
        let deserialized: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tool, "create_issue");
        assert_eq!(deserialized.duration_ms, 100);
    }

    // === ExecutionStats ===

    #[test]
    fn test_execution_stats_default() {
        let stats = ExecutionStats::default();
        assert_eq!(stats.total_duration_ms, 0);
        assert_eq!(stats.monty_execution_ms, 0);
        assert_eq!(stats.external_calls, 0);
        assert_eq!(stats.memory_peak_bytes, 0);
        assert_eq!(stats.steps_executed, 0);
    }

    // === ExecutionResult ===

    #[test]
    fn test_execution_result_builder() {
        let result = ExecutionResult::success(serde_json::json!({"status": "ok"}))
            .with_stdout("Hello".to_string())
            .with_stderr("Warning".to_string());

        assert_eq!(result.output, serde_json::json!({"status": "ok"}));
        assert_eq!(result.stdout, "Hello");
        assert_eq!(result.stderr, "Warning");
    }

    #[test]
    fn test_execution_result_with_trace() {
        let call = ToolCall::new(
            "echo".to_string(),
            serde_json::json!({}),
        );
        let result =
            ExecutionResult::success(serde_json::json!("ok")).with_trace(vec![call]);

        assert_eq!(result.trace.len(), 1);
        assert_eq!(result.trace[0].tool, "echo");
    }

    #[test]
    fn test_execution_result_with_stats() {
        let stats = ExecutionStats {
            total_duration_ms: 100,
            monty_execution_ms: 50,
            external_calls: 2,
            memory_peak_bytes: 1024,
            steps_executed: 10,
        };
        let result =
            ExecutionResult::success(serde_json::json!("ok")).with_stats(stats);

        assert_eq!(result.stats.total_duration_ms, 100);
        assert_eq!(result.stats.external_calls, 2);
    }

    #[test]
    fn test_execution_result_full_chain() {
        let stats = ExecutionStats {
            total_duration_ms: 200,
            ..Default::default()
        };
        let result = ExecutionResult::success(serde_json::json!(42))
            .with_stdout("out".to_string())
            .with_stderr("err".to_string())
            .with_trace(vec![])
            .with_stats(stats);

        assert_eq!(result.output, serde_json::json!(42));
        assert_eq!(result.stdout, "out");
        assert_eq!(result.stderr, "err");
        assert!(result.trace.is_empty());
        assert_eq!(result.stats.total_duration_ms, 200);
    }

    // === ResourceLimits ===

    #[test]
    fn test_resource_limits_default() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.max_execution_time_ms, 30_000);
        assert_eq!(limits.max_memory_bytes, 50 * 1024 * 1024);
        assert_eq!(limits.max_stack_depth, 100);
        assert_eq!(limits.max_external_calls, 50);
        assert_eq!(limits.max_code_length, 10_000);
    }

    #[test]
    fn test_resource_limits_custom() {
        let limits = ResourceLimits {
            max_execution_time_ms: 5000,
            max_memory_bytes: 1024,
            max_stack_depth: 10,
            max_external_calls: 5,
            max_code_length: 100,
        };
        assert_eq!(limits.max_execution_time_ms, 5000);
        assert_eq!(limits.max_code_length, 100);
    }

    #[test]
    fn test_resource_limits_serialization() {
        let limits = ResourceLimits::default();
        let json = serde_json::to_string(&limits).unwrap();
        let deserialized: ResourceLimits = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.max_execution_time_ms, 30_000);
    }

    // === ExecutionLimits ===

    #[test]
    fn test_execution_limits_default() {
        let limits = ExecutionLimits::default();
        assert_eq!(limits.timeout_ms, 30_000);
        assert_eq!(limits.max_concurrent, 5);
    }

    #[test]
    fn test_execution_limits_serialization() {
        let limits = ExecutionLimits {
            timeout_ms: 5000,
            max_concurrent: 2,
        };
        let json = serde_json::to_string(&limits).unwrap();
        let deserialized: ExecutionLimits = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.timeout_ms, 5000);
        assert_eq!(deserialized.max_concurrent, 2);
    }

    // === MontygateError ===

    #[test]
    fn test_error_display_messages() {
        assert_eq!(
            MontygateError::Execution("fail".into()).to_string(),
            "Execution error: fail"
        );
        assert_eq!(
            MontygateError::ToolNotFound("x".into()).to_string(),
            "Tool not found: x"
        );
        assert_eq!(
            MontygateError::PolicyViolation("denied".into()).to_string(),
            "Policy violation: denied"
        );
        assert_eq!(
            MontygateError::RateLimitExceeded("10/min".into()).to_string(),
            "Rate limit exceeded: 10/min"
        );
        assert_eq!(
            MontygateError::ResourceLimitExceeded("mem".into()).to_string(),
            "Resource limit exceeded: mem"
        );
        assert_eq!(
            MontygateError::Timeout("5s".into()).to_string(),
            "Timeout: 5s"
        );
        assert_eq!(
            MontygateError::MaxRetries("3 attempts".into()).to_string(),
            "Max retries exceeded: 3 attempts"
        );
        assert_eq!(
            MontygateError::Validation("bad input".into()).to_string(),
            "Validation error: bad input"
        );
        assert_eq!(
            MontygateError::TypeCheck("bad type".into()).to_string(),
            "Type check error: bad type"
        );
        assert_eq!(
            MontygateError::Parse("syntax".into()).to_string(),
            "Parse error: syntax"
        );
        assert_eq!(
            MontygateError::Configuration("missing".into()).to_string(),
            "Configuration error: missing"
        );
        assert_eq!(
            MontygateError::Interrupted.to_string(),
            "External call interrupted"
        );
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: MontygateError = io_err.into();
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn test_error_from_serde_json() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err: MontygateError = json_err.into();
        assert!(err.to_string().contains("Serialization error"));
    }

    // === RetryConfig ===

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.base_delay_ms, 100);
    }

    #[test]
    fn test_retry_config_serialization() {
        let config = RetryConfig {
            max_retries: 5,
            base_delay_ms: 200,
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: RetryConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.max_retries, 5);
        assert_eq!(deserialized.base_delay_ms, 200);
    }

    // === Policy types ===

    #[test]
    fn test_policy_config_default() {
        let config = PolicyConfig::default();
        assert!(config.rules.is_empty());
        assert!(matches!(config.defaults.action, PolicyAction::Allow));
    }

    #[test]
    fn test_policy_defaults_default() {
        let defaults = PolicyDefaults::default();
        assert!(matches!(defaults.action, PolicyAction::Allow));
    }

    #[test]
    fn test_policy_action_serialization() {
        let allow_json = serde_json::to_string(&PolicyAction::Allow).unwrap();
        assert_eq!(allow_json, "\"allow\"");
        let deny_json = serde_json::to_string(&PolicyAction::Deny).unwrap();
        assert_eq!(deny_json, "\"deny\"");
        let approve_json = serde_json::to_string(&PolicyAction::RequireApproval).unwrap();
        assert_eq!(approve_json, "\"require_approval\"");
    }

    #[test]
    fn test_policy_rule_serialization() {
        let rule = PolicyRule {
            match_pattern: "*.delete_*".to_string(),
            action: PolicyAction::Deny,
            rate_limit: None,
        };
        let json = serde_json::to_string(&rule).unwrap();
        let deserialized: PolicyRule = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.match_pattern, "*.delete_*");
        assert!(deserialized.rate_limit.is_none());
    }

    #[test]
    fn test_policy_rule_with_rate_limit() {
        let rule = PolicyRule {
            match_pattern: "github.*".to_string(),
            action: PolicyAction::Allow,
            rate_limit: Some("10/min".to_string()),
        };
        let json = serde_json::to_string(&rule).unwrap();
        let deserialized: PolicyRule = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.rate_limit, Some("10/min".to_string()));
    }

    // === RunProgramInput ===

    #[test]
    fn test_run_program_input_defaults() {
        let json = serde_json::json!({
            "code": "print('hello')"
        });

        let input: RunProgramInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.code, "print('hello')");
        assert!(input.inputs.is_empty());
        assert!(input.type_check);
    }

    #[test]
    fn test_run_program_input_full() {
        let json = serde_json::json!({
            "code": "x + y",
            "inputs": {"x": 1, "y": 2},
            "type_check": false
        });
        let input: RunProgramInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.code, "x + y");
        assert_eq!(input.inputs.len(), 2);
        assert!(!input.type_check);
    }

    // === ExternalCall ===

    #[test]
    fn test_external_call() {
        let call = ExternalCall {
            function_name: "tool".to_string(),
            arguments: serde_json::json!({"name": "create_issue"}),
        };
        assert_eq!(call.function_name, "tool");
    }

    // === ExecutionState ===

    #[test]
    fn test_execution_state_variants() {
        let running = ExecutionState::Running;
        assert!(matches!(running, ExecutionState::Running));

        let call = ExternalCall {
            function_name: "tool".to_string(),
            arguments: serde_json::json!({}),
        };
        let paused = ExecutionState::Paused(call);
        assert!(matches!(paused, ExecutionState::Paused(_)));

        let result = ExecutionResult::success(serde_json::json!("done"));
        let complete = ExecutionState::Complete(result);
        assert!(matches!(complete, ExecutionState::Complete(_)));

        let error = ExecutionState::Error("boom".to_string());
        assert!(matches!(error, ExecutionState::Error(_)));
    }

    // === ExecutionSnapshot ===

    #[test]
    fn test_execution_snapshot() {
        let snapshot = ExecutionSnapshot {
            id: "snap-1".to_string(),
            state: vec![1, 2, 3],
            pending_call: Some(ExternalCall {
                function_name: "tool".to_string(),
                arguments: serde_json::json!({}),
            }),
            trace: vec![],
        };
        assert_eq!(snapshot.id, "snap-1");
        assert!(snapshot.pending_call.is_some());
        assert!(snapshot.trace.is_empty());
    }

    #[test]
    fn test_execution_snapshot_no_pending() {
        let snapshot = ExecutionSnapshot {
            id: "snap-2".to_string(),
            state: vec![],
            pending_call: None,
            trace: vec![ToolCall::new(
                "test_tool".to_string(),
                serde_json::json!({}),
            )],
        };
        assert!(snapshot.pending_call.is_none());
        assert_eq!(snapshot.trace.len(), 1);
    }

    // === ToolDefinition ===

    #[test]
    fn test_tool_definition_serialization() {
        let def = ToolDefinition {
            name: "create_issue".to_string(),
            description: Some("Create a GitHub issue".to_string()),
            input_schema: serde_json::json!({"type": "object"}),
        };
        let json = serde_json::to_string(&def).unwrap();
        let deserialized: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "create_issue");
        assert_eq!(
            deserialized.description,
            Some("Create a GitHub issue".to_string())
        );
    }

    #[test]
    fn test_tool_definition_no_description() {
        let def = ToolDefinition {
            name: "test".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let json = serde_json::to_string(&def).unwrap();
        let deserialized: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert!(deserialized.description.is_none());
    }
}
