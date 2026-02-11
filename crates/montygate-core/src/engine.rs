use crate::types::{ExecutionResult, ResourceLimits, Result, RunProgramInput, ToolCall};
use crate::MontyGateError;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info};

/// Trait for dispatching external tool calls
#[async_trait::async_trait]
pub trait ToolDispatcher: Send + Sync + std::fmt::Debug {
    /// Dispatch a tool call to the appropriate downstream server
    async fn dispatch(&self, tool_name: &str, args: serde_json::Value) -> Result<serde_json::Value>;
}

/// Execution engine for running Monty programs
///
/// This is a trait-based abstraction that allows different implementations:
/// - MockEngine: For testing
/// - MontyEngine: Real Monty execution (when available)
/// - DryRunEngine: Validates without executing
#[async_trait::async_trait]
pub trait ExecutionEngine: Send + Sync + std::fmt::Debug {
    /// Execute a program with the given inputs
    async fn execute(
        &self,
        input: RunProgramInput,
        dispatcher: Arc<dyn ToolDispatcher>,
    ) -> Result<ExecutionResult>;

    /// Validate code without executing
    fn validate(&self, code: &str) -> Result<()>;

    /// Get resource limits
    fn limits(&self) -> ResourceLimits;
}

/// Mock execution engine for testing
///
/// This engine simulates Monty execution by parsing the code and extracting
/// tool calls from special comments. It's useful for testing the integration
/// without requiring the full Monty runtime.
#[derive(Debug, Clone)]
pub struct MockEngine {
    limits: ResourceLimits,
}

impl MockEngine {
    pub fn new(limits: ResourceLimits) -> Self {
        Self { limits }
    }

    /// Parse tool calls from code comments
    /// Format: # TOOL: tool_name {json_args}
    fn parse_tool_calls(&self, code: &str) -> Vec<(String, serde_json::Value)> {
        let mut calls = Vec::new();
        let prefix = "# TOOL:";

        for line in code.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix(prefix) {
                let content = rest.trim();
                if let Some(space_idx) = content.find(' ') {
                    let tool_name = &content[..space_idx];
                    let args_str = &content[space_idx + 1..];
                    if let Ok(args) = serde_json::from_str(args_str) {
                        calls.push((tool_name.to_string(), args));
                    } else {
                        calls.push((tool_name.to_string(), serde_json::json!({})));
                    }
                } else if !content.is_empty() {
                    calls.push((content.to_string(), serde_json::json!({})));
                }
            }
        }

        calls
    }
}

#[async_trait::async_trait]
impl ExecutionEngine for MockEngine {
    async fn execute(
        &self,
        input: RunProgramInput,
        dispatcher: Arc<dyn ToolDispatcher>,
    ) -> Result<ExecutionResult> {
        let start = Instant::now();
        let mut trace = Vec::new();

        // Validate code length
        if input.code.len() > self.limits.max_code_length {
            return Err(MontyGateError::ResourceLimitExceeded(format!(
                "Code exceeds maximum length of {} characters",
                self.limits.max_code_length
            )));
        }

        info!("Starting mock execution");

        // Parse tool calls from comments
        let tool_calls = self.parse_tool_calls(&input.code);

        // Check external call limit
        if tool_calls.len() > self.limits.max_external_calls {
            return Err(MontyGateError::ResourceLimitExceeded(format!(
                "Too many external calls: {} (max: {})",
                tool_calls.len(),
                self.limits.max_external_calls
            )));
        }

        // Execute each tool call
        for (tool_name, args) in tool_calls {
            let call_start = Instant::now();
            debug!("Dispatching tool call: {}", tool_name);

            match dispatcher.dispatch(&tool_name, args.clone()).await {
                Ok(result) => {
                    let duration = call_start.elapsed().as_millis() as u64;
                    let parts: Vec<&str> = tool_name.split('.').collect();
                    let (server, tool) = if parts.len() >= 2 {
                        (parts[0].to_string(), parts[1..].join("."))
                    } else {
                        ("unknown".to_string(), tool_name.clone())
                    };

                    let call = ToolCall::new(server, tool, args)
                        .with_result(result, duration);
                    trace.push(call);
                }
                Err(e) => {
                    let duration = call_start.elapsed().as_millis() as u64;
                    let parts: Vec<&str> = tool_name.split('.').collect();
                    let (server, tool) = if parts.len() >= 2 {
                        (parts[0].to_string(), parts[1..].join("."))
                    } else {
                        ("unknown".to_string(), tool_name.clone())
                    };

                    let call = ToolCall::new(server, tool, args)
                        .with_error(e.to_string(), duration);
                    trace.push(call);
                }
            }
        }

        let total_duration = start.elapsed().as_millis() as u64;
        let trace_len = trace.len();

        info!(
            "Mock execution completed in {}ms with {} tool calls",
            total_duration,
            trace_len
        );

        Ok(ExecutionResult {
            output: serde_json::json!({
                "status": "completed",
                "tool_calls": trace_len
            }),
            stdout: String::new(),
            stderr: String::new(),
            trace,
            stats: crate::types::ExecutionStats {
                total_duration_ms: total_duration,
                monty_execution_ms: 0,
                external_calls: trace_len,
                memory_peak_bytes: 0,
                steps_executed: 0,
            },
        })
    }

    fn validate(&self, code: &str) -> Result<()> {
        // Basic validation: check for syntax-like issues
        let mut paren_depth = 0;
        let mut brace_depth = 0;
        let mut bracket_depth = 0;

        for (line_num, line) in code.lines().enumerate() {
            for ch in line.chars() {
                match ch {
                    '(' => paren_depth += 1,
                    ')' => paren_depth -= 1,
                    '{' => brace_depth += 1,
                    '}' => brace_depth -= 1,
                    '[' => bracket_depth += 1,
                    ']' => bracket_depth -= 1,
                    _ => {}
                }

                if paren_depth < 0 || brace_depth < 0 || bracket_depth < 0 {
                    return Err(MontyGateError::Parse(format!(
                        "Unbalanced brackets at line {}",
                        line_num + 1
                    )));
                }
            }
        }

        if paren_depth != 0 || brace_depth != 0 || bracket_depth != 0 {
            return Err(MontyGateError::Parse(
                "Unbalanced brackets in code".to_string(),
            ));
        }

        Ok(())
    }

    fn limits(&self) -> ResourceLimits {
        self.limits.clone()
    }
}

impl Default for MockEngine {
    fn default() -> Self {
        Self::new(ResourceLimits::default())
    }
}

/// Simple async tool dispatcher for testing
#[derive(Default)]
pub struct SimpleDispatcher {
    callbacks: HashMap<String, Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value> + Send + Sync>>,
}

impl std::fmt::Debug for SimpleDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimpleDispatcher")
            .field("registered_tools", &self.callbacks.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl SimpleDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<F>(&mut self, tool_name: &str, callback: F)
    where
        F: Fn(serde_json::Value) -> Result<serde_json::Value> + Send + Sync + 'static,
    {
        self.callbacks.insert(tool_name.to_string(), Box::new(callback));
    }
}

#[async_trait::async_trait]
impl ToolDispatcher for SimpleDispatcher {
    async fn dispatch(&self, tool_name: &str, args: serde_json::Value) -> Result<serde_json::Value> {
        if let Some(callback) = self.callbacks.get(tool_name) {
            callback(args)
        } else {
            Err(MontyGateError::ToolNotFound(tool_name.to_string()))
        }
    }
}

/// Execution engine manager that can switch between implementations
pub struct EngineManager {
    engine: Arc<dyn ExecutionEngine>,
}

impl EngineManager {
    pub fn new(engine: Arc<dyn ExecutionEngine>) -> Self {
        Self { engine }
    }

    pub fn with_mock(limits: ResourceLimits) -> Self {
        Self::new(Arc::new(MockEngine::new(limits)))
    }

    pub async fn execute(
        &self,
        input: RunProgramInput,
        dispatcher: Arc<dyn ToolDispatcher>,
    ) -> Result<ExecutionResult> {
        self.engine.execute(input, dispatcher).await
    }

    pub fn validate(&self, code: &str) -> Result<()> {
        self.engine.validate(code)
    }

    pub fn limits(&self) -> ResourceLimits {
        self.engine.limits()
    }

    /// Get the underlying engine as a trait object
    pub fn engine(&self) -> Arc<dyn ExecutionEngine> {
        self.engine.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === MockEngine ===

    #[test]
    fn test_mock_engine_new() {
        let limits = ResourceLimits {
            max_code_length: 500,
            ..Default::default()
        };
        let engine = MockEngine::new(limits.clone());
        assert_eq!(engine.limits().max_code_length, 500);
    }

    #[test]
    fn test_mock_engine_default() {
        let engine = MockEngine::default();
        let limits = engine.limits();
        assert_eq!(limits.max_execution_time_ms, 30_000);
        assert_eq!(limits.max_code_length, 10_000);
    }

    #[tokio::test]
    async fn test_mock_engine_basic_execution() {
        let engine = MockEngine::default();
        let mut dispatcher = SimpleDispatcher::new();

        dispatcher.register("test.echo", |args| Ok(args));

        let input = RunProgramInput {
            code: r#"
                # TOOL: test.echo {"message": "hello"}
            "#
            .to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = engine
            .execute(input, Arc::new(dispatcher))
            .await
            .unwrap();

        assert_eq!(result.trace.len(), 1);
        assert_eq!(result.trace[0].server, "test");
        assert_eq!(result.trace[0].tool, "echo");
        assert!(result.trace[0].result.is_some());
        assert!(result.trace[0].error.is_none());
    }

    #[tokio::test]
    async fn test_mock_engine_no_tool_calls() {
        let engine = MockEngine::default();
        let dispatcher = SimpleDispatcher::new();

        let input = RunProgramInput {
            code: "x = 1 + 2\nprint(x)".to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();
        assert!(result.trace.is_empty());
        assert_eq!(result.stats.external_calls, 0);
        assert_eq!(
            result.output,
            serde_json::json!({"status": "completed", "tool_calls": 0})
        );
    }

    #[tokio::test]
    async fn test_mock_engine_multiple_calls() {
        let engine = MockEngine::default();
        let mut dispatcher = SimpleDispatcher::new();

        dispatcher.register("github.create_issue", |_args| {
            Ok(serde_json::json!({"id": 123}))
        });
        dispatcher.register("slack.post_message", |_args| {
            Ok(serde_json::json!({"ok": true}))
        });

        let input = RunProgramInput {
            code: r#"
                # TOOL: github.create_issue {"title": "Test"}
                # TOOL: slack.post_message {"text": "Created issue"}
            "#
            .to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = engine
            .execute(input, Arc::new(dispatcher))
            .await
            .unwrap();

        assert_eq!(result.trace.len(), 2);
        assert_eq!(result.trace[0].server, "github");
        assert_eq!(result.trace[0].tool, "create_issue");
        assert_eq!(result.trace[1].server, "slack");
        assert_eq!(result.trace[1].tool, "post_message");
        assert_eq!(result.stats.external_calls, 2);
    }

    #[tokio::test]
    async fn test_mock_engine_tool_without_server_prefix() {
        let engine = MockEngine::default();
        let mut dispatcher = SimpleDispatcher::new();
        dispatcher.register("plain_tool", |_| Ok(serde_json::json!("ok")));

        let input = RunProgramInput {
            code: "# TOOL: plain_tool {}".to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();
        assert_eq!(result.trace.len(), 1);
        // Without a dot, server should be "unknown"
        assert_eq!(result.trace[0].server, "unknown");
        assert_eq!(result.trace[0].tool, "plain_tool");
    }

    #[test]
    fn test_mock_engine_validation_valid() {
        let engine = MockEngine::default();
        assert!(engine.validate("def test():\n    pass").is_ok());
        assert!(engine.validate("x = [1, 2, {\"a\": 3}]").is_ok());
        assert!(engine.validate("").is_ok());
    }

    #[test]
    fn test_mock_engine_validation_unbalanced_parens() {
        let engine = MockEngine::default();
        assert!(engine.validate("def test(:\n    pass").is_err());
    }

    #[test]
    fn test_mock_engine_validation_unbalanced_braces() {
        let engine = MockEngine::default();
        assert!(engine.validate("def test():\n    {{").is_err());
    }

    #[test]
    fn test_mock_engine_validation_unbalanced_brackets() {
        let engine = MockEngine::default();
        assert!(engine.validate("x = [1, 2").is_err());
    }

    #[test]
    fn test_mock_engine_validation_extra_closing() {
        let engine = MockEngine::default();
        assert!(engine.validate(")").is_err());
        assert!(engine.validate("}").is_err());
        assert!(engine.validate("]").is_err());
    }

    #[tokio::test]
    async fn test_mock_engine_code_length_limit() {
        let limits = ResourceLimits {
            max_code_length: 10,
            ..Default::default()
        };
        let engine = MockEngine::new(limits);
        let dispatcher = SimpleDispatcher::new();

        let input = RunProgramInput {
            code: "# This is a long comment that exceeds the limit".to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = engine.execute(input, Arc::new(dispatcher)).await;
        assert!(matches!(
            result.unwrap_err(),
            MontyGateError::ResourceLimitExceeded(_)
        ));
    }

    #[tokio::test]
    async fn test_mock_engine_external_call_limit() {
        let limits = ResourceLimits {
            max_external_calls: 2,
            ..Default::default()
        };
        let engine = MockEngine::new(limits);
        let dispatcher = SimpleDispatcher::new();

        let input = RunProgramInput {
            code: r#"
                # TOOL: tool1 {}
                # TOOL: tool2 {}
                # TOOL: tool3 {}
            "#
            .to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = engine.execute(input, Arc::new(dispatcher)).await;
        assert!(matches!(
            result.unwrap_err(),
            MontyGateError::ResourceLimitExceeded(_)
        ));
    }

    #[tokio::test]
    async fn test_tool_dispatch_error_recorded_in_trace() {
        let engine = MockEngine::default();
        let dispatcher = SimpleDispatcher::new();

        let input = RunProgramInput {
            code: "# TOOL: unknown.tool {}".to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();
        assert_eq!(result.trace.len(), 1);
        assert!(result.trace[0].error.is_some());
        assert!(result.trace[0].result.is_none());
        assert_eq!(result.trace[0].server, "unknown");
        assert_eq!(result.trace[0].tool, "tool");
    }

    // === parse_tool_calls ===

    #[test]
    fn test_parse_tool_calls() {
        let engine = MockEngine::default();

        let code = r#"
            # TOOL: github.create_issue {"title": "Bug"}
            # TOOL: slack.post_message {"text": "Hello"}
            # Regular comment
            x = 1 + 2
            # TOOL: test.tool {}
        "#;

        let calls = engine.parse_tool_calls(code);
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].0, "github.create_issue");
        assert_eq!(calls[1].0, "slack.post_message");
        assert_eq!(calls[2].0, "test.tool");
    }

    #[test]
    fn test_parse_tool_calls_no_args() {
        let engine = MockEngine::default();
        let calls = engine.parse_tool_calls("# TOOL: my_tool");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "my_tool");
        assert_eq!(calls[0].1, serde_json::json!({}));
    }

    #[test]
    fn test_parse_tool_calls_invalid_json_args() {
        let engine = MockEngine::default();
        let calls = engine.parse_tool_calls("# TOOL: my_tool {invalid json}");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "my_tool");
        assert_eq!(calls[0].1, serde_json::json!({}));
    }

    #[test]
    fn test_parse_tool_calls_empty_code() {
        let engine = MockEngine::default();
        let calls = engine.parse_tool_calls("");
        assert!(calls.is_empty());
    }

    #[test]
    fn test_parse_tool_calls_only_regular_comments() {
        let engine = MockEngine::default();
        let calls = engine.parse_tool_calls("# This is a comment\n# Another comment");
        assert!(calls.is_empty());
    }

    #[test]
    fn test_parse_tool_calls_empty_tool_prefix() {
        let engine = MockEngine::default();
        let calls = engine.parse_tool_calls("# TOOL:");
        // Empty content after trimming, so no call
        assert!(calls.is_empty());
    }

    // === SimpleDispatcher ===

    #[test]
    fn test_simple_dispatcher_debug() {
        let mut dispatcher = SimpleDispatcher::new();
        dispatcher.register("test.echo", |args| Ok(args));
        let debug_str = format!("{:?}", dispatcher);
        assert!(debug_str.contains("SimpleDispatcher"));
        assert!(debug_str.contains("test.echo"));
    }

    #[tokio::test]
    async fn test_simple_dispatcher_registered_tool() {
        let mut dispatcher = SimpleDispatcher::new();
        dispatcher.register("echo", |args| Ok(args));

        let result = dispatcher
            .dispatch("echo", serde_json::json!({"msg": "hi"}))
            .await
            .unwrap();
        assert_eq!(result, serde_json::json!({"msg": "hi"}));
    }

    #[tokio::test]
    async fn test_simple_dispatcher_tool_not_found() {
        let dispatcher = SimpleDispatcher::new();
        let result = dispatcher
            .dispatch("missing", serde_json::json!({}))
            .await;
        assert!(matches!(
            result.unwrap_err(),
            MontyGateError::ToolNotFound(_)
        ));
    }

    // === EngineManager ===

    #[test]
    fn test_engine_manager_with_mock() {
        let limits = ResourceLimits {
            max_code_length: 500,
            ..Default::default()
        };
        let manager = EngineManager::with_mock(limits);
        assert_eq!(manager.limits().max_code_length, 500);
    }

    #[test]
    fn test_engine_manager_validate() {
        let manager = EngineManager::with_mock(ResourceLimits::default());
        assert!(manager.validate("x = 1").is_ok());
        assert!(manager.validate("x = (").is_err());
    }

    #[test]
    fn test_engine_manager_limits() {
        let limits = ResourceLimits {
            max_external_calls: 99,
            ..Default::default()
        };
        let manager = EngineManager::with_mock(limits);
        assert_eq!(manager.limits().max_external_calls, 99);
    }

    #[test]
    fn test_engine_manager_engine_ref() {
        let manager = EngineManager::with_mock(ResourceLimits::default());
        let engine = manager.engine();
        // Should be able to call validate through the engine ref
        assert!(engine.validate("x = 1").is_ok());
    }

    #[tokio::test]
    async fn test_engine_manager_execute() {
        let manager = EngineManager::with_mock(ResourceLimits::default());
        let mut dispatcher = SimpleDispatcher::new();
        dispatcher.register("test.echo", |args| Ok(args));

        let input = RunProgramInput {
            code: "# TOOL: test.echo {}".to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = manager.execute(input, Arc::new(dispatcher)).await.unwrap();
        assert_eq!(result.trace.len(), 1);
    }

    #[test]
    fn test_engine_manager_new() {
        let engine = Arc::new(MockEngine::default());
        let manager = EngineManager::new(engine);
        assert!(manager.validate("x = 1").is_ok());
    }
}