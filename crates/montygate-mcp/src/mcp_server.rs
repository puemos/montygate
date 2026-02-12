use montygate_core::{
    ExecutionEngine, ExecutionResult, MontyGateError, RunProgramInput, ToolDispatcher,
    ToolRegistry,
};
use std::sync::Arc;
use tracing::{debug, error, info};

// Re-export rmcp types that users might need
pub use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    tool, tool_router,
    transport::stdio,
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
};

use rmcp::handler::server::tool::ToolCallContext;
use rmcp::service::RequestContext;

/// Input parameters for the run_program tool
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RunProgramRequest {
    /// Python code to execute in the sandboxed interpreter
    #[schemars(description = "Python code that can call tools via the tool() function")]
    pub code: String,

    /// Optional input variables to pass to the code
    #[serde(default)]
    #[schemars(description = "Input variables available to the code")]
    pub inputs: Option<serde_json::Value>,

    /// Whether to type-check the code before execution
    #[serde(default = "default_true")]
    #[schemars(description = "Enable type checking before execution")]
    pub type_check: bool,
}

fn default_true() -> bool {
    true
}

/// Input parameters for the call_tool escape hatch
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CallToolRequest {
    /// Server name (e.g. "github")
    #[schemars(description = "Name of the downstream MCP server")]
    pub server: String,

    /// Tool name (e.g. "create_issue")
    #[schemars(description = "Name of the tool on the server")]
    pub tool: String,

    /// Arguments to pass to the tool
    #[serde(default)]
    #[schemars(description = "JSON arguments for the tool")]
    pub args: serde_json::Value,
}

/// Output from the run_program tool
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct RunProgramOutput {
    /// Execution result
    pub result: serde_json::Value,
    /// Standard output from the code
    pub stdout: String,
    /// Standard error from the code
    pub stderr: String,
    /// Execution trace showing all tool calls
    pub trace: Vec<ToolCallInfo>,
    /// Execution statistics
    pub stats: ExecutionStats,
}

/// Information about a tool call in the execution trace
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct ToolCallInfo {
    pub server: String,
    pub tool: String,
    pub arguments: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Execution statistics
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct ExecutionStats {
    pub total_duration_ms: u64,
    pub external_calls: usize,
}

/// MontyGate MCP Server that exposes the run_program tool
///
/// This is the upstream MCP server that MCP clients (like Claude Desktop) connect to.
/// It provides a single tool: `run_program` that executes Monty code.
#[derive(Clone, Debug)]
pub struct MontyGateMcpServer {
    engine: Arc<dyn ExecutionEngine>,
    dispatcher: Arc<dyn ToolDispatcher>,
    registry: Arc<ToolRegistry>,
    tool_router: ToolRouter<Self>,
}

impl MontyGateMcpServer {
    /// Create a new MCP server with the given engine, dispatcher, and registry
    pub fn new(
        engine: Arc<dyn ExecutionEngine>,
        dispatcher: Arc<dyn ToolDispatcher>,
        registry: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            engine,
            dispatcher,
            registry,
            tool_router: Self::tool_router(),
        }
    }

    /// Build the server with stdio transport and run it
    ///
    /// This method sets up the MCP server with stdio transport and runs it,
    /// waiting for connections from MCP clients.
    pub async fn run_stdio(self) -> anyhow::Result<()> {
        info!("Starting MontyGate MCP server with stdio transport");

        let service = self.serve(stdio()).await.map_err(|e| {
            anyhow::anyhow!("Failed to start MCP server: {}", e)
        })?;

        info!("MCP server running, waiting for connections...");

        // Wait for the server to complete (typically when stdin closes)
        service.waiting().await.map_err(|e| {
            anyhow::anyhow!("MCP server error: {}", e)
        })?;

        info!("MCP server shut down");
        Ok(())
    }

    /// Build the tool catalog listing available downstream tools
    fn build_tool_catalog(&self) -> Option<String> {
        let mut tools = self.registry.list_tools();
        tools.sort();

        if tools.is_empty() {
            return None;
        }

        let mut catalog = String::new();
        for name in &tools {
            if let Ok(route) = self.registry.resolve(name) {
                let desc = route
                    .definition
                    .description
                    .as_deref()
                    .unwrap_or("No description");
                catalog.push_str(&format!("  - {} - {}\n", name, desc));
            }
        }

        Some(catalog)
    }

    /// Build a dynamic description for run_program including available tools
    fn build_run_program_description(&self) -> String {
        match self.build_tool_catalog() {
            None => {
                "Execute Python code in a sandboxed interpreter. \
                 The code can call tools via the tool() function. \
                 No downstream tools are currently connected."
                    .to_string()
            }
            Some(catalog) => {
                let count = self.registry.list_tools().len();
                format!(
                    "Execute Python code in a sandboxed interpreter with access to {} tools.\n\n\
                     Available tools:\n{}\n\
                     Usage: result = tool(\"server.tool_name\", arg1=val1, ...)\n\
                     The code runs in a secure sandbox with resource limits.",
                    count, catalog
                )
            }
        }
    }

    /// Build a dynamic description for call_tool including available tools
    fn build_call_tool_description(&self) -> String {
        match self.build_tool_catalog() {
            None => {
                "Directly invoke a single tool on a downstream MCP server. \
                 No downstream tools are currently connected."
                    .to_string()
            }
            Some(catalog) => {
                format!(
                    "Directly invoke a single tool on a downstream MCP server. \
                     Use run_program for multi-tool orchestration; use call_tool for simple one-shot calls.\n\n\
                     Available tools:\n{}",
                    catalog
                )
            }
        }
    }
}

// This impl block with tools generates the Self::tool_router() method
#[tool_router]
impl MontyGateMcpServer {
    /// Execute Python code in a sandboxed interpreter
    ///
    /// The code can call tools via the `tool()` function:
    /// result = tool('server.tool_name', arg1=val1, ...)
    ///
    /// Available tools depend on the configured downstream MCP servers.
    #[tool(name = "run_program", description = "Execute Python code in a sandboxed interpreter. The code can call tools via the tool() function: result = tool('server.tool_name', arg1=val1, ...). The code runs in a secure sandbox with resource limits.")]
    async fn run_program(&self, request: Parameters<RunProgramRequest>) -> String {
        let params = request.0;
        debug!("Received run_program call with {} bytes of code", params.code.len());

        let input = RunProgramInput {
            code: params.code,
            inputs: parse_inputs(params.inputs),
            type_check: params.type_check,
        };

        match self.execute_program(input).await {
            Ok(result) => {
                let output = execution_result_to_output(result);
                serde_json::to_string_pretty(&output).unwrap_or_default()
            }
            Err(e) => {
                error!("Execution failed: {}", e);
                format!("Execution error: {}", e)
            }
        }
    }

    /// Directly invoke a single tool on a downstream server without writing Python.
    /// Use this as a fallback when run_program is not needed (single tool call, no orchestration).
    #[tool(name = "call_tool", description = "Directly invoke a single tool on a downstream MCP server. Use run_program for multi-tool orchestration; use call_tool for simple one-shot calls.")]
    async fn call_tool(&self, request: Parameters<CallToolRequest>) -> String {
        let params = request.0;
        let qualified_name = format!("{}.{}", params.server, params.tool);
        debug!("Received call_tool for '{}'", qualified_name);

        match self.dispatcher.dispatch(&qualified_name, params.args).await {
            Ok(result) => {
                serde_json::to_string_pretty(&result).unwrap_or_default()
            }
            Err(e) => {
                error!("call_tool '{}' failed: {}", qualified_name, e);
                format!("Tool call error: {}", e)
            }
        }
    }
}

// Additional impl block for helper methods
impl MontyGateMcpServer {
    /// Execute the program using the engine
    async fn execute_program(&self, input: RunProgramInput) -> std::result::Result<ExecutionResult, MontyGateError> {
        self.engine.execute(input, self.dispatcher.clone()).await
    }
}

// Manual ServerHandler implementation (instead of #[tool_handler])
// so we can dynamically modify the run_program tool description
impl ServerHandler for MontyGateMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "MontyGate MCP Server: Execute Python code with access to downstream MCP tools. \
                Use run_program to execute code. Tools from connected servers can be called via \
                the tool() function.".to_string()
            ),
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        async move {
            let mut tools = self.tool_router.list_all();

            // Dynamically update tool descriptions with available tools
            for tool in &mut tools {
                match tool.name.as_ref() {
                    "run_program" => {
                        tool.description = Some(self.build_run_program_description().into());
                    }
                    "call_tool" => {
                        tool.description = Some(self.build_call_tool_description().into());
                    }
                    _ => {}
                }
            }

            Ok(ListToolsResult {
                tools,
                meta: None,
                next_cursor: None,
            })
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        async move {
            let tcc = ToolCallContext::new(self, request, context);
            self.tool_router.call(tcc).await
        }
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        let mut tool = self.tool_router.get(name).cloned();

        // Apply dynamic descriptions
        if let Some(ref mut t) = tool {
            match t.name.as_ref() {
                "run_program" => {
                    t.description = Some(self.build_run_program_description().into());
                }
                "call_tool" => {
                    t.description = Some(self.build_call_tool_description().into());
                }
                _ => {}
            }
        }

        tool
    }
}

/// Parse inputs from JSON value to HashMap
fn parse_inputs(inputs: Option<serde_json::Value>) -> std::collections::HashMap<String, serde_json::Value> {
    match inputs {
        Some(serde_json::Value::Object(map)) => {
            map.into_iter().collect()
        }
        _ => std::collections::HashMap::new(),
    }
}

/// Convert ExecutionResult to RunProgramOutput
fn execution_result_to_output(result: ExecutionResult) -> RunProgramOutput {
    RunProgramOutput {
        result: result.output,
        stdout: result.stdout,
        stderr: result.stderr,
        trace: result.trace.into_iter().map(|tc| ToolCallInfo {
            server: tc.server,
            tool: tc.tool,
            arguments: tc.arguments,
            result: tc.result,
            error: tc.error,
            duration_ms: tc.duration_ms,
        }).collect(),
        stats: ExecutionStats {
            total_duration_ms: result.stats.total_duration_ms,
            external_calls: result.stats.external_calls,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use montygate_core::{MockEngine, SimpleDispatcher, ToolCall, ToolDefinition};

    fn create_test_server() -> MontyGateMcpServer {
        let engine = Arc::new(MockEngine::default());
        let dispatcher = Arc::new(SimpleDispatcher::new());
        let registry = Arc::new(ToolRegistry::new());
        MontyGateMcpServer::new(engine, dispatcher, registry)
    }

    fn create_test_server_with_tools() -> MontyGateMcpServer {
        let engine = Arc::new(MockEngine::default());
        let dispatcher = Arc::new(SimpleDispatcher::new());
        let registry = Arc::new(ToolRegistry::new());

        registry
            .register_server_tools(
                "github",
                vec![
                    ToolDefinition {
                        name: "create_issue".to_string(),
                        description: Some("Create a GitHub issue".to_string()),
                        input_schema: serde_json::json!({
                            "type": "object",
                            "properties": {
                                "repo": {"type": "string"},
                                "title": {"type": "string"}
                            },
                            "required": ["repo", "title"]
                        }),
                    },
                    ToolDefinition {
                        name: "list_repos".to_string(),
                        description: Some("List GitHub repositories".to_string()),
                        input_schema: serde_json::json!({"type": "object"}),
                    },
                ],
            )
            .unwrap();

        MontyGateMcpServer::new(engine, dispatcher, registry)
    }

    // === MontyGateMcpServer ===

    #[test]
    fn test_server_creation() {
        let server = create_test_server();
        let debug = format!("{:?}", server);
        assert!(debug.contains("MontyGateMcpServer"));
    }

    #[test]
    fn test_server_clone() {
        let server = create_test_server();
        let cloned = server.clone();
        let _ = format!("{:?}", cloned);
    }

    // === Dynamic description ===

    #[test]
    fn test_dynamic_description_empty_registry() {
        let server = create_test_server();
        let desc = server.build_run_program_description();
        assert!(desc.contains("No downstream tools are currently connected"));
        let call_desc = server.build_call_tool_description();
        assert!(call_desc.contains("No downstream tools are currently connected"));
    }

    #[test]
    fn test_dynamic_description_with_tools() {
        let server = create_test_server_with_tools();
        let desc = server.build_run_program_description();
        assert!(desc.contains("github.create_issue"));
        assert!(desc.contains("github.list_repos"));
        assert!(desc.contains("Create a GitHub issue"));
        assert!(desc.contains("List GitHub repositories"));
        assert!(desc.contains("2 tools"));
        assert!(desc.contains("tool("));

        let call_desc = server.build_call_tool_description();
        assert!(call_desc.contains("github.create_issue"));
        assert!(call_desc.contains("call_tool"));
    }

    // === parse_inputs ===

    #[test]
    fn test_parse_inputs_object() {
        let inputs = Some(serde_json::json!({
            "key1": "value1",
            "key2": 42
        }));
        let result = parse_inputs(inputs);
        assert_eq!(result.len(), 2);
        assert_eq!(result.get("key1").unwrap().as_str().unwrap(), "value1");
        assert_eq!(result.get("key2").unwrap().as_i64().unwrap(), 42);
    }

    #[test]
    fn test_parse_inputs_none() {
        let result = parse_inputs(None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_inputs_non_object() {
        let result = parse_inputs(Some(serde_json::json!("just a string")));
        assert!(result.is_empty());

        let result = parse_inputs(Some(serde_json::json!([1, 2, 3])));
        assert!(result.is_empty());

        let result = parse_inputs(Some(serde_json::json!(42)));
        assert!(result.is_empty());

        let result = parse_inputs(Some(serde_json::json!(null)));
        assert!(result.is_empty());
    }

    // === execution_result_to_output ===

    #[test]
    fn test_execution_result_to_output_empty() {
        let result = montygate_core::ExecutionResult::success(serde_json::json!("done"));
        let output = execution_result_to_output(result);

        assert_eq!(output.result, serde_json::json!("done"));
        assert_eq!(output.stdout, "");
        assert_eq!(output.stderr, "");
        assert!(output.trace.is_empty());
        assert_eq!(output.stats.total_duration_ms, 0);
        assert_eq!(output.stats.external_calls, 0);
    }

    #[test]
    fn test_execution_result_to_output_with_trace() {
        let call = ToolCall::new(
            "github".to_string(),
            "create_issue".to_string(),
            serde_json::json!({"title": "Bug"}),
        )
        .with_result(serde_json::json!({"id": 123}), 50);

        let result = montygate_core::ExecutionResult::success(serde_json::json!({"status": "ok"}))
            .with_stdout("hello\n".to_string())
            .with_stderr("warn\n".to_string())
            .with_trace(vec![call])
            .with_stats(montygate_core::ExecutionStats {
                total_duration_ms: 150,
                monty_execution_ms: 100,
                external_calls: 1,
                memory_peak_bytes: 2048,
                steps_executed: 5,
            });

        let output = execution_result_to_output(result);

        assert_eq!(output.result, serde_json::json!({"status": "ok"}));
        assert_eq!(output.stdout, "hello\n");
        assert_eq!(output.stderr, "warn\n");
        assert_eq!(output.trace.len(), 1);
        assert_eq!(output.trace[0].server, "github");
        assert_eq!(output.trace[0].tool, "create_issue");
        assert_eq!(
            output.trace[0].arguments,
            serde_json::json!({"title": "Bug"})
        );
        assert_eq!(
            output.trace[0].result,
            Some(serde_json::json!({"id": 123}))
        );
        assert!(output.trace[0].error.is_none());
        assert_eq!(output.trace[0].duration_ms, 50);
        assert_eq!(output.stats.total_duration_ms, 150);
        assert_eq!(output.stats.external_calls, 1);
    }

    #[test]
    fn test_execution_result_to_output_with_error_trace() {
        let call = ToolCall::new(
            "db".to_string(),
            "query".to_string(),
            serde_json::json!({}),
        )
        .with_error("timeout".to_string(), 3000);

        let result = montygate_core::ExecutionResult::success(serde_json::json!(null))
            .with_trace(vec![call]);

        let output = execution_result_to_output(result);
        assert_eq!(output.trace.len(), 1);
        assert!(output.trace[0].result.is_none());
        assert_eq!(output.trace[0].error, Some("timeout".to_string()));
        assert_eq!(output.trace[0].duration_ms, 3000);
    }

    // === RunProgramRequest ===

    #[test]
    fn test_run_program_request_deserialization() {
        let json = serde_json::json!({
            "code": "print('hi')",
            "inputs": {"x": 1},
            "type_check": false
        });
        let req: RunProgramRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.code, "print('hi')");
        assert!(req.inputs.is_some());
        assert!(!req.type_check);
    }

    #[test]
    fn test_run_program_request_defaults() {
        let json = serde_json::json!({
            "code": "x = 1"
        });
        let req: RunProgramRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.code, "x = 1");
        assert!(req.inputs.is_none());
        assert!(req.type_check); // default true
    }

    // === RunProgramOutput ===

    #[test]
    fn test_run_program_output_serialization() {
        let output = RunProgramOutput {
            result: serde_json::json!(42),
            stdout: "out".to_string(),
            stderr: "".to_string(),
            trace: vec![ToolCallInfo {
                server: "s".to_string(),
                tool: "t".to_string(),
                arguments: serde_json::json!({}),
                result: Some(serde_json::json!("ok")),
                error: None,
                duration_ms: 10,
            }],
            stats: ExecutionStats {
                total_duration_ms: 100,
                external_calls: 1,
            },
        };

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"result\":42"));
        assert!(json.contains("\"stdout\":\"out\""));
        assert!(json.contains("\"total_duration_ms\":100"));
    }

    // === ToolCallInfo ===

    #[test]
    fn test_tool_call_info_serialization() {
        let info = ToolCallInfo {
            server: "github".to_string(),
            tool: "create_issue".to_string(),
            arguments: serde_json::json!({"title": "Bug"}),
            result: Some(serde_json::json!({"id": 1})),
            error: None,
            duration_ms: 42,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"server\":\"github\""));
        assert!(json.contains("\"duration_ms\":42"));
    }

    // === ExecutionStats ===

    #[test]
    fn test_execution_stats_serialization() {
        let stats = ExecutionStats {
            total_duration_ms: 500,
            external_calls: 3,
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("500"));
        assert!(json.contains("3"));
    }
}
