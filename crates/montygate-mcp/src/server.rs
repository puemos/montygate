use montygate_core::{
    ExecutionEngine, ExecutionResult, MontyGateError, Result, RunProgramInput,
    ToolDispatcher,
};
use std::sync::Arc;
use tracing::{debug, info, instrument};

/// Server handler for the upstream MCP server
/// 
/// This is what MCP clients (like Claude Desktop) connect to
pub struct MontyGateServerHandler {
    engine: Arc<dyn ExecutionEngine>,
    dispatcher: Arc<dyn ToolDispatcher>,
}

impl std::fmt::Debug for MontyGateServerHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MontyGateServerHandler")
            .field("engine", &"<dyn ExecutionEngine>")
            .field("dispatcher", &"<dyn ToolDispatcher>")
            .finish()
    }
}

impl MontyGateServerHandler {
    pub fn new(
        engine: Arc<dyn ExecutionEngine>,
        dispatcher: Arc<dyn ToolDispatcher>,
    ) -> Self {
        Self {
            engine,
            dispatcher,
        }
    }

    /// Handle a run_program tool call
    #[instrument(skip(self, input))]
    pub async fn handle_run_program(&self, input: RunProgramInput) -> Result<ExecutionResult> {
        info!("Handling run_program call");
        
        // Type check if requested
        if input.type_check {
            debug!("Type-checking code before execution");
            self.engine.validate(&input.code)?;
        }
        
        // Execute the program
        let result = self.engine.execute(input, self.dispatcher.clone()).await?;
        
        info!("run_program completed successfully");
        Ok(result)
    }

    /// Generate the tool description for run_program
    pub fn generate_run_program_description(&self, tool_catalog: &str) -> String {
        format!(
            r#"run_program: Execute Python code in a sandboxed interpreter.

The code can call tools via the `tool()` function.

Available tools:
{}

Call tools with: result = await tool('server.tool_name', arg1=val1, ...)
All tool calls are async. Use await.

The code runs in a secure sandbox with:
- No filesystem access
- No network access (except through whitelisted tools)
- Resource limits (memory, execution time, call count)
- Complete execution trace for debugging"#,
            tool_catalog
        )
    }
}

/// Configuration for MCP transports
#[derive(Debug, Clone)]
pub enum McpTransport {
    Stdio,
    Sse { host: String, port: u16 },
    StreamableHttp { host: String, port: u16 },
}

/// Builder for creating the MontyGate MCP server
#[derive(Default)]
pub struct McpServerBuilder {
    transport: Option<McpTransport>,
    engine: Option<Arc<dyn ExecutionEngine>>,
    dispatcher: Option<Arc<dyn ToolDispatcher>>,
}

impl std::fmt::Debug for McpServerBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerBuilder")
            .field("transport", &self.transport)
            .field("engine", &"<dyn ExecutionEngine>")
            .field("dispatcher", &"<dyn ToolDispatcher>")
            .finish()
    }
}

impl McpServerBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn transport(mut self, transport: McpTransport) -> Self {
        self.transport = Some(transport);
        self
    }

    pub fn engine(mut self, engine: Arc<dyn ExecutionEngine>) -> Self {
        self.engine = Some(engine);
        self
    }

    pub fn dispatcher(mut self, dispatcher: Arc<dyn ToolDispatcher>) -> Self {
        self.dispatcher = Some(dispatcher);
        self
    }

    pub fn build(self) -> Result<MontyGateServerHandler> {
        Ok(MontyGateServerHandler::new(
            self.engine.ok_or_else(|| {
                MontyGateError::Configuration("Engine is required".to_string())
            })?,
            self.dispatcher.ok_or_else(|| {
                MontyGateError::Configuration("Dispatcher is required".to_string())
            })?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use montygate_core::{MockEngine, SimpleDispatcher};
    use std::collections::HashMap;

    // === MontyGateServerHandler ===

    #[tokio::test]
    async fn test_server_handler_run_program() {
        let engine = Arc::new(MockEngine::default());
        let dispatcher = Arc::new(SimpleDispatcher::new());
        let handler = MontyGateServerHandler::new(engine, dispatcher);

        let input = RunProgramInput {
            code: "# Simple test".to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = handler.handle_run_program(input).await;
        assert!(result.is_ok());
        let exec_result = result.unwrap();
        assert!(exec_result.trace.is_empty());
    }

    #[tokio::test]
    async fn test_server_handler_with_tool_calls() {
        let engine = Arc::new(MockEngine::default());
        let mut dispatcher = SimpleDispatcher::new();
        dispatcher.register("test.echo", |args| Ok(args));
        let handler = MontyGateServerHandler::new(engine, Arc::new(dispatcher));

        let input = RunProgramInput {
            code: "# TOOL: test.echo {\"msg\": \"hi\"}".to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = handler.handle_run_program(input).await.unwrap();
        assert_eq!(result.trace.len(), 1);
    }

    #[tokio::test]
    async fn test_server_handler_type_check_disabled() {
        let engine = Arc::new(MockEngine::default());
        let dispatcher = Arc::new(SimpleDispatcher::new());
        let handler = MontyGateServerHandler::new(engine, dispatcher);

        // This code has unbalanced parens but type_check is false
        let input = RunProgramInput {
            code: "def test(:".to_string(),
            inputs: HashMap::new(),
            type_check: false,
        };

        let result = handler.handle_run_program(input).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_server_handler_type_check_failure() {
        let engine = Arc::new(MockEngine::default());
        let dispatcher = Arc::new(SimpleDispatcher::new());
        let handler = MontyGateServerHandler::new(engine, dispatcher);

        let input = RunProgramInput {
            code: "def test(:".to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = handler.handle_run_program(input).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_description() {
        let engine = Arc::new(MockEngine::default());
        let dispatcher = Arc::new(SimpleDispatcher::new());
        let handler = MontyGateServerHandler::new(engine, dispatcher);

        let catalog = "- github.create_issue\n- slack.post_message";
        let desc = handler.generate_run_program_description(catalog);

        assert!(desc.contains("run_program"));
        assert!(desc.contains("github.create_issue"));
        assert!(desc.contains("slack.post_message"));
        assert!(desc.contains("sandbox"));
        assert!(desc.contains("tool()"));
    }

    #[test]
    fn test_generate_description_empty_catalog() {
        let engine = Arc::new(MockEngine::default());
        let dispatcher = Arc::new(SimpleDispatcher::new());
        let handler = MontyGateServerHandler::new(engine, dispatcher);

        let desc = handler.generate_run_program_description("");
        assert!(desc.contains("run_program"));
    }

    #[test]
    fn test_server_handler_debug() {
        let engine = Arc::new(MockEngine::default());
        let dispatcher = Arc::new(SimpleDispatcher::new());
        let handler = MontyGateServerHandler::new(engine, dispatcher);
        let debug = format!("{:?}", handler);
        assert!(debug.contains("MontyGateServerHandler"));
    }

    // === McpTransport ===

    #[test]
    fn test_mcp_transport_variants() {
        let stdio = McpTransport::Stdio;
        assert!(matches!(stdio, McpTransport::Stdio));

        let sse = McpTransport::Sse {
            host: "localhost".into(),
            port: 3000,
        };
        assert!(matches!(sse, McpTransport::Sse { .. }));

        let http = McpTransport::StreamableHttp {
            host: "0.0.0.0".into(),
            port: 8080,
        };
        assert!(matches!(http, McpTransport::StreamableHttp { .. }));
    }

    // === McpServerBuilder ===

    #[test]
    fn test_mcp_server_builder_success() {
        let engine = Arc::new(MockEngine::default()) as Arc<dyn ExecutionEngine>;
        let dispatcher = Arc::new(SimpleDispatcher::new()) as Arc<dyn ToolDispatcher>;

        let handler = McpServerBuilder::new()
            .transport(McpTransport::Stdio)
            .engine(engine)
            .dispatcher(dispatcher)
            .build();

        assert!(handler.is_ok());
    }

    #[test]
    fn test_mcp_server_builder_missing_engine() {
        let dispatcher = Arc::new(SimpleDispatcher::new()) as Arc<dyn ToolDispatcher>;

        let result = McpServerBuilder::new()
            .transport(McpTransport::Stdio)
            .dispatcher(dispatcher)
            .build();

        assert!(result.is_err());
    }

    #[test]
    fn test_mcp_server_builder_missing_dispatcher() {
        let engine = Arc::new(MockEngine::default()) as Arc<dyn ExecutionEngine>;

        let result = McpServerBuilder::new()
            .transport(McpTransport::Stdio)
            .engine(engine)
            .build();

        assert!(result.is_err());
    }

    #[test]
    fn test_mcp_server_builder_debug() {
        let builder = McpServerBuilder::new().transport(McpTransport::Stdio);
        let debug = format!("{:?}", builder);
        assert!(debug.contains("McpServerBuilder"));
    }
}