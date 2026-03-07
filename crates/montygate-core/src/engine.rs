use crate::MontygateError;
use crate::types::{ExecutionResult, ResourceLimits, Result, RunProgramInput, ToolCall};
use monty::{
    CollectStringPrint, ExcType, ExternalResult, LimitedTracker, MontyException, MontyObject,
    MontyRun, RunProgress,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

/// Trait for dispatching external tool calls
#[async_trait::async_trait]
pub trait ToolDispatcher: Send + Sync + std::fmt::Debug {
    /// Dispatch a tool call to the appropriate handler
    async fn dispatch(&self, tool_name: &str, args: serde_json::Value)
    -> Result<serde_json::Value>;
}

/// Execution engine for running Monty programs
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

        if input.code.len() > self.limits.max_code_length {
            return Err(MontygateError::ResourceLimitExceeded(format!(
                "Code exceeds maximum length of {} characters",
                self.limits.max_code_length
            )));
        }

        info!("Starting mock execution");

        let tool_calls = self.parse_tool_calls(&input.code);

        if tool_calls.len() > self.limits.max_external_calls {
            return Err(MontygateError::ResourceLimitExceeded(format!(
                "Too many external calls: {} (max: {})",
                tool_calls.len(),
                self.limits.max_external_calls
            )));
        }

        for (tool_name, args) in tool_calls {
            let call_start = Instant::now();
            debug!("Dispatching tool call: {}", tool_name);

            match dispatcher.dispatch(&tool_name, args.clone()).await {
                Ok(result) => {
                    let duration = call_start.elapsed().as_millis() as u64;
                    let call = ToolCall::new(tool_name, args).with_result(result, duration);
                    trace.push(call);
                }
                Err(e) => {
                    let duration = call_start.elapsed().as_millis() as u64;
                    let call = ToolCall::new(tool_name, args).with_error(e.to_string(), duration);
                    trace.push(call);
                }
            }
        }

        let total_duration = start.elapsed().as_millis() as u64;
        let trace_len = trace.len();

        info!(
            "Mock execution completed in {}ms with {} tool calls",
            total_duration, trace_len
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
                    return Err(MontygateError::Parse(format!(
                        "Unbalanced brackets at line {}",
                        line_num + 1
                    )));
                }
            }
        }

        if paren_depth != 0 || brace_depth != 0 || bracket_depth != 0 {
            return Err(MontygateError::Parse(
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

/// A tool call request sent from the Monty blocking thread to the async dispatcher.
struct ToolCallRequest {
    tool_name: String,
    args: serde_json::Value,
    response_tx: oneshot::Sender<std::result::Result<serde_json::Value, String>>,
}

/// A batch of tool call requests for parallel dispatch.
struct BatchToolCallRequest {
    calls: Vec<(String, serde_json::Value)>,
    response_tx: oneshot::Sender<Vec<std::result::Result<serde_json::Value, String>>>,
}

/// Messages sent from the Monty blocking thread to the async dispatcher.
enum ToolCallMessage {
    Single(ToolCallRequest),
    Batch(BatchToolCallRequest),
}

/// Real execution engine backed by Monty (RustPython-based sandboxed interpreter).
#[derive(Debug, Clone)]
pub struct MontyEngine {
    limits: ResourceLimits,
}

impl MontyEngine {
    pub fn new(limits: ResourceLimits) -> Self {
        Self { limits }
    }

    /// Parse a tool call from Monty's FunctionCall args/kwargs.
    fn parse_tool_call(
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> std::result::Result<(String, serde_json::Value), String> {
        let tool_name = match args.first() {
            Some(MontyObject::String(s)) => s.clone(),
            Some(other) => {
                return Err(format!(
                    "tool() first argument must be a string (tool name), got: {}",
                    other.py_repr()
                ));
            }
            None => {
                return Err("tool() requires at least one argument (tool name)".to_string());
            }
        };

        let tool_args = if !kwargs.is_empty() {
            let mut map = serde_json::Map::new();
            for (k, v) in kwargs {
                let key = match k {
                    MontyObject::String(s) => s.clone(),
                    other => other.py_repr(),
                };
                map.insert(key, crate::convert::monty_to_json(v));
            }
            serde_json::Value::Object(map)
        } else if let Some(dict_arg) = args.get(1) {
            crate::convert::monty_to_json(dict_arg)
        } else {
            serde_json::json!({})
        };

        Ok((tool_name, tool_args))
    }

    /// Parse batch_tools arguments from Monty's FunctionCall args.
    fn parse_batch_tool_args(
        args: &[MontyObject],
    ) -> std::result::Result<Vec<(String, serde_json::Value)>, String> {
        let list = match args.first() {
            Some(MontyObject::List(items)) => items,
            Some(MontyObject::Tuple(items)) => items,
            Some(other) => {
                return Err(format!(
                    "batch_tools() argument must be a list of (name, args) tuples, got: {}",
                    other.py_repr()
                ));
            }
            None => {
                return Err(
                    "batch_tools() requires one argument: a list of (name, args) tuples"
                        .to_string(),
                );
            }
        };

        let mut calls = Vec::with_capacity(list.len());
        for (i, item) in list.iter().enumerate() {
            let tuple_items = match item {
                MontyObject::Tuple(items) | MontyObject::List(items) => items,
                other => {
                    return Err(format!(
                        "batch_tools() item {} must be a (name, args) tuple, got: {}",
                        i,
                        other.py_repr()
                    ));
                }
            };

            if tuple_items.len() != 2 {
                return Err(format!(
                    "batch_tools() item {} must have exactly 2 elements (name, args), got {}",
                    i,
                    tuple_items.len()
                ));
            }

            let name = match &tuple_items[0] {
                MontyObject::String(s) => s.clone(),
                other => {
                    return Err(format!(
                        "batch_tools() item {} name must be a string, got: {}",
                        i,
                        other.py_repr()
                    ));
                }
            };

            let args_value = crate::convert::monty_to_json(&tuple_items[1]);
            calls.push((name, args_value));
        }

        Ok(calls)
    }
}

#[async_trait::async_trait]
impl ExecutionEngine for MontyEngine {
    async fn execute(
        &self,
        input: RunProgramInput,
        dispatcher: Arc<dyn ToolDispatcher>,
    ) -> Result<ExecutionResult> {
        let start = Instant::now();

        if input.code.len() > self.limits.max_code_length {
            return Err(MontygateError::ResourceLimitExceeded(format!(
                "Code exceeds maximum length of {} characters",
                self.limits.max_code_length
            )));
        }

        let max_external_calls = self.limits.max_external_calls;

        info!("Starting Monty execution");

        let (call_tx, mut call_rx) = mpsc::channel::<ToolCallMessage>(1);

        let monty_limits = monty::ResourceLimits {
            max_memory: Some(self.limits.max_memory_bytes),
            max_duration: Some(std::time::Duration::from_millis(
                self.limits.max_execution_time_ms,
            )),
            max_recursion_depth: Some(self.limits.max_stack_depth),
            gc_interval: None,
            max_allocations: None,
        };

        let code = input.code.clone();
        let inputs = input.inputs.clone();

        let monty_handle = tokio::task::spawn_blocking(move || {
            run_monty_blocking(&code, monty_limits, max_external_calls, call_tx, inputs)
        });

        let mut trace = Vec::new();
        while let Some(msg) = call_rx.recv().await {
            match msg {
                ToolCallMessage::Single(req) => {
                    let call_start = Instant::now();
                    debug!("Dispatching tool call: {}", req.tool_name);

                    match dispatcher.dispatch(&req.tool_name, req.args.clone()).await {
                        Ok(result) => {
                            let duration = call_start.elapsed().as_millis() as u64;
                            let call = ToolCall::new(req.tool_name, req.args)
                                .with_result(result.clone(), duration);
                            trace.push(call);
                            let _ = req.response_tx.send(Ok(result));
                        }
                        Err(e) => {
                            let duration = call_start.elapsed().as_millis() as u64;
                            let err_str = e.to_string();
                            let call = ToolCall::new(req.tool_name, req.args)
                                .with_error(err_str.clone(), duration);
                            trace.push(call);
                            let _ = req.response_tx.send(Err(err_str));
                        }
                    }
                }
                ToolCallMessage::Batch(batch) => {
                    debug!("Dispatching batch of {} tool calls", batch.calls.len());

                    let results =
                        futures::future::join_all(batch.calls.iter().map(|(name, args)| {
                            let d = dispatcher.clone();
                            let name = name.clone();
                            let args = args.clone();
                            async move { d.dispatch(&name, args).await }
                        }))
                        .await;

                    for (i, result) in results.iter().enumerate() {
                        let (name, args) = &batch.calls[i];

                        match result {
                            Ok(value) => {
                                let call = ToolCall::new(name.clone(), args.clone())
                                    .with_result(value.clone(), 0);
                                trace.push(call);
                            }
                            Err(e) => {
                                let call = ToolCall::new(name.clone(), args.clone())
                                    .with_error(e.to_string(), 0);
                                trace.push(call);
                            }
                        }
                    }

                    let response: Vec<std::result::Result<serde_json::Value, String>> = results
                        .into_iter()
                        .map(|r| r.map_err(|e| e.to_string()))
                        .collect();

                    let _ = batch.response_tx.send(response);
                }
            }
        }

        let monty_result = monty_handle.await.map_err(|e| {
            MontygateError::Execution(format!("Monty execution thread panicked: {}", e))
        })?;

        let total_duration = start.elapsed().as_millis() as u64;
        let monty_duration = monty_result.execution_time_ms;
        let trace_len = trace.len();

        info!(
            "Monty execution completed in {}ms with {} tool calls",
            total_duration, trace_len
        );

        match monty_result.error {
            Some(err) if !monty_result.success => {
                warn!("Monty execution error: {}", err);
                Ok(ExecutionResult {
                    output: serde_json::json!({
                        "status": "error",
                        "error": err,
                    }),
                    stdout: monty_result.stdout,
                    stderr: err.clone(),
                    trace,
                    stats: crate::types::ExecutionStats {
                        total_duration_ms: total_duration,
                        monty_execution_ms: monty_duration,
                        external_calls: trace_len,
                        memory_peak_bytes: 0,
                        steps_executed: 0,
                    },
                })
            }
            _ => Ok(ExecutionResult {
                output: monty_result.result,
                stdout: monty_result.stdout,
                stderr: String::new(),
                trace,
                stats: crate::types::ExecutionStats {
                    total_duration_ms: total_duration,
                    monty_execution_ms: monty_duration,
                    external_calls: trace_len,
                    memory_peak_bytes: 0,
                    steps_executed: 0,
                },
            }),
        }
    }

    fn validate(&self, code: &str) -> Result<()> {
        let mut paren_depth = 0i32;
        let mut brace_depth = 0i32;
        let mut bracket_depth = 0i32;

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
                    return Err(MontygateError::Parse(format!(
                        "Unbalanced brackets at line {}",
                        line_num + 1
                    )));
                }
            }
        }

        if paren_depth != 0 || brace_depth != 0 || bracket_depth != 0 {
            return Err(MontygateError::Parse(
                "Unbalanced brackets in code".to_string(),
            ));
        }

        Ok(())
    }

    fn limits(&self) -> ResourceLimits {
        self.limits.clone()
    }
}

impl Default for MontyEngine {
    fn default() -> Self {
        Self::new(ResourceLimits::default())
    }
}

/// Result from the blocking Monty execution thread.
struct MontyBlockingResult {
    success: bool,
    result: serde_json::Value,
    stdout: String,
    error: Option<String>,
    execution_time_ms: u64,
}

/// Run the Monty interpreter in a blocking context.
fn run_monty_blocking(
    code: &str,
    limits: monty::ResourceLimits,
    max_external_calls: usize,
    call_tx: mpsc::Sender<ToolCallMessage>,
    inputs: HashMap<String, serde_json::Value>,
) -> MontyBlockingResult {
    let start = Instant::now();

    let ext_fns = vec!["tool".to_string(), "batch_tools".to_string()];

    let mut input_keys: Vec<String> = inputs.keys().cloned().collect();
    input_keys.sort();
    let input_values: Vec<MontyObject> = input_keys
        .iter()
        .map(|k| crate::convert::json_to_monty(&inputs[k]))
        .collect();

    let runner = match MontyRun::new(code.to_string(), "program.py", input_keys, ext_fns) {
        Ok(r) => r,
        Err(e) => {
            return MontyBlockingResult {
                success: false,
                result: serde_json::Value::Null,
                stdout: String::new(),
                error: Some(format_exception(&e)),
                execution_time_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    let tracker = LimitedTracker::new(limits);
    let mut print = CollectStringPrint::new();

    let mut progress = match runner.start(input_values, tracker, &mut print) {
        Ok(p) => p,
        Err(e) => {
            return MontyBlockingResult {
                success: false,
                result: serde_json::Value::Null,
                stdout: print.output().to_string(),
                error: Some(format_exception(&e)),
                execution_time_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    let mut external_calls: usize = 0;

    loop {
        match progress {
            RunProgress::Complete(value) => {
                let json_value = crate::convert::monty_to_json(&value);
                return MontyBlockingResult {
                    success: true,
                    result: json_value,
                    stdout: print.output().to_string(),
                    error: None,
                    execution_time_ms: start.elapsed().as_millis() as u64,
                };
            }
            RunProgress::FunctionCall {
                function_name,
                args,
                kwargs,
                call_id: _,
                state,
            } => {
                let ext_result = match function_name.as_str() {
                    "tool" => {
                        external_calls += 1;
                        if external_calls > max_external_calls {
                            let err = MontyException::new(
                                ExcType::RuntimeError,
                                Some(format!(
                                    "External call limit exceeded ({} calls, max: {})",
                                    external_calls, max_external_calls
                                )),
                            );
                            match state.run(ExternalResult::Error(err), &mut print) {
                                Ok(p) => {
                                    progress = p;
                                    continue;
                                }
                                Err(e) => {
                                    return MontyBlockingResult {
                                        success: false,
                                        result: serde_json::Value::Null,
                                        stdout: print.output().to_string(),
                                        error: Some(format_exception(&e)),
                                        execution_time_ms: start.elapsed().as_millis() as u64,
                                    };
                                }
                            }
                        }

                        let (tool_name, tool_args) =
                            match MontyEngine::parse_tool_call(&args, &kwargs) {
                                Ok(parsed) => parsed,
                                Err(err_msg) => {
                                    let err =
                                        MontyException::new(ExcType::TypeError, Some(err_msg));
                                    match state.run(ExternalResult::Error(err), &mut print) {
                                        Ok(p) => {
                                            progress = p;
                                            continue;
                                        }
                                        Err(e) => {
                                            return MontyBlockingResult {
                                                success: false,
                                                result: serde_json::Value::Null,
                                                stdout: print.output().to_string(),
                                                error: Some(format_exception(&e)),
                                                execution_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                    }
                                }
                            };

                        let (resp_tx, resp_rx) = oneshot::channel();

                        let request = ToolCallRequest {
                            tool_name: tool_name.clone(),
                            args: tool_args,
                            response_tx: resp_tx,
                        };

                        if call_tx
                            .blocking_send(ToolCallMessage::Single(request))
                            .is_err()
                        {
                            return MontyBlockingResult {
                                success: false,
                                result: serde_json::Value::Null,
                                stdout: print.output().to_string(),
                                error: Some(
                                    "Execution cancelled: dispatcher channel closed".to_string(),
                                ),
                                execution_time_ms: start.elapsed().as_millis() as u64,
                            };
                        }

                        match resp_rx.blocking_recv() {
                            Ok(Ok(value)) => {
                                ExternalResult::Return(crate::convert::json_to_monty(&value))
                            }
                            Ok(Err(err_msg)) => ExternalResult::Error(MontyException::new(
                                ExcType::RuntimeError,
                                Some(format!("Tool call failed: {err_msg}")),
                            )),
                            Err(_) => ExternalResult::Error(MontyException::new(
                                ExcType::RuntimeError,
                                Some("Tool call cancelled: response channel closed".to_string()),
                            )),
                        }
                    }
                    "batch_tools" => {
                        let calls = match MontyEngine::parse_batch_tool_args(&args) {
                            Ok(calls) => calls,
                            Err(err_msg) => {
                                let err = MontyException::new(ExcType::TypeError, Some(err_msg));
                                match state.run(ExternalResult::Error(err), &mut print) {
                                    Ok(p) => {
                                        progress = p;
                                        continue;
                                    }
                                    Err(e) => {
                                        return MontyBlockingResult {
                                            success: false,
                                            result: serde_json::Value::Null,
                                            stdout: print.output().to_string(),
                                            error: Some(format_exception(&e)),
                                            execution_time_ms: start.elapsed().as_millis() as u64,
                                        };
                                    }
                                }
                            }
                        };

                        external_calls += calls.len();
                        if external_calls > max_external_calls {
                            let err = MontyException::new(
                                ExcType::RuntimeError,
                                Some(format!(
                                    "External call limit exceeded ({} calls, max: {})",
                                    external_calls, max_external_calls
                                )),
                            );
                            match state.run(ExternalResult::Error(err), &mut print) {
                                Ok(p) => {
                                    progress = p;
                                    continue;
                                }
                                Err(e) => {
                                    return MontyBlockingResult {
                                        success: false,
                                        result: serde_json::Value::Null,
                                        stdout: print.output().to_string(),
                                        error: Some(format_exception(&e)),
                                        execution_time_ms: start.elapsed().as_millis() as u64,
                                    };
                                }
                            }
                        }

                        let (resp_tx, resp_rx) = oneshot::channel();

                        let batch_request = BatchToolCallRequest {
                            calls,
                            response_tx: resp_tx,
                        };

                        if call_tx
                            .blocking_send(ToolCallMessage::Batch(batch_request))
                            .is_err()
                        {
                            return MontyBlockingResult {
                                success: false,
                                result: serde_json::Value::Null,
                                stdout: print.output().to_string(),
                                error: Some(
                                    "Execution cancelled: dispatcher channel closed".to_string(),
                                ),
                                execution_time_ms: start.elapsed().as_millis() as u64,
                            };
                        }

                        match resp_rx.blocking_recv() {
                            Ok(results) => {
                                let items: Vec<MontyObject> = results
                                    .into_iter()
                                    .map(|r| match r {
                                        Ok(value) => crate::convert::json_to_monty(&value),
                                        Err(err_msg) => {
                                            let pairs: Vec<(MontyObject, MontyObject)> = vec![(
                                                MontyObject::String("error".to_string()),
                                                MontyObject::String(err_msg),
                                            )];
                                            MontyObject::Dict(pairs.into())
                                        }
                                    })
                                    .collect();
                                ExternalResult::Return(MontyObject::List(items))
                            }
                            Err(_) => ExternalResult::Error(MontyException::new(
                                ExcType::RuntimeError,
                                Some(
                                    "Batch tool call cancelled: response channel closed"
                                        .to_string(),
                                ),
                            )),
                        }
                    }
                    _ => {
                        let err = MontyException::new(
                            ExcType::NameError,
                            Some(format!("Unknown function: {function_name}")),
                        );
                        match state.run(ExternalResult::Error(err), &mut print) {
                            Ok(p) => {
                                progress = p;
                                continue;
                            }
                            Err(e) => {
                                return MontyBlockingResult {
                                    success: false,
                                    result: serde_json::Value::Null,
                                    stdout: print.output().to_string(),
                                    error: Some(format_exception(&e)),
                                    execution_time_ms: start.elapsed().as_millis() as u64,
                                };
                            }
                        }
                    }
                };

                match state.run(ext_result, &mut print) {
                    Ok(p) => progress = p,
                    Err(e) => {
                        return MontyBlockingResult {
                            success: false,
                            result: serde_json::Value::Null,
                            stdout: print.output().to_string(),
                            error: Some(format_exception(&e)),
                            execution_time_ms: start.elapsed().as_millis() as u64,
                        };
                    }
                }
            }
            RunProgress::OsCall { state, .. } => {
                let err = MontyException::new(
                    ExcType::RuntimeError,
                    Some(
                        "OS operations (filesystem, network, environment) are not available in the sandbox."
                            .to_string(),
                    ),
                );
                match state.run(ExternalResult::Error(err), &mut print) {
                    Ok(p) => progress = p,
                    Err(e) => {
                        return MontyBlockingResult {
                            success: false,
                            result: serde_json::Value::Null,
                            stdout: print.output().to_string(),
                            error: Some(format_exception(&e)),
                            execution_time_ms: start.elapsed().as_millis() as u64,
                        };
                    }
                }
            }
            RunProgress::ResolveFutures(_) => {
                return MontyBlockingResult {
                    success: false,
                    result: serde_json::Value::Null,
                    stdout: print.output().to_string(),
                    error: Some("Async operations are not supported in the sandbox.".to_string()),
                    execution_time_ms: start.elapsed().as_millis() as u64,
                };
            }
        }
    }
}

fn format_exception(e: &MontyException) -> String {
    e.to_string()
}

/// Simple async tool dispatcher for testing
#[derive(Default)]
pub struct SimpleDispatcher {
    callbacks:
        HashMap<String, Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value> + Send + Sync>>,
}

impl std::fmt::Debug for SimpleDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimpleDispatcher")
            .field(
                "registered_tools",
                &self.callbacks.keys().collect::<Vec<_>>(),
            )
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
        self.callbacks
            .insert(tool_name.to_string(), Box::new(callback));
    }
}

#[async_trait::async_trait]
impl ToolDispatcher for SimpleDispatcher {
    async fn dispatch(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value> {
        if let Some(callback) = self.callbacks.get(tool_name) {
            callback(args)
        } else {
            Err(MontygateError::ToolNotFound(tool_name.to_string()))
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

    pub fn with_monty(limits: ResourceLimits) -> Self {
        Self::new(Arc::new(MontyEngine::new(limits)))
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

        let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();

        assert_eq!(result.trace.len(), 1);
        assert_eq!(result.trace[0].tool, "test.echo");
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
    }

    #[tokio::test]
    async fn test_mock_engine_multiple_calls() {
        let engine = MockEngine::default();
        let mut dispatcher = SimpleDispatcher::new();

        dispatcher.register("create_issue", |_args| Ok(serde_json::json!({"id": 123})));
        dispatcher.register("post_message", |_args| Ok(serde_json::json!({"ok": true})));

        let input = RunProgramInput {
            code: r#"
                # TOOL: create_issue {"title": "Test"}
                # TOOL: post_message {"text": "Created issue"}
            "#
            .to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();

        assert_eq!(result.trace.len(), 2);
        assert_eq!(result.trace[0].tool, "create_issue");
        assert_eq!(result.trace[1].tool, "post_message");
        assert_eq!(result.stats.external_calls, 2);
    }

    #[test]
    fn test_mock_engine_validation_valid() {
        let engine = MockEngine::default();
        assert!(engine.validate("def test():\n    pass").is_ok());
        assert!(engine.validate("x = [1, 2, {\"a\": 3}]").is_ok());
        assert!(engine.validate("").is_ok());
    }

    #[test]
    fn test_mock_engine_validation_unbalanced() {
        let engine = MockEngine::default();
        assert!(engine.validate("def test(:\n    pass").is_err());
        assert!(engine.validate("x = [1, 2").is_err());
        assert!(engine.validate(")").is_err());
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
            MontygateError::ResourceLimitExceeded(_)
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
            MontygateError::ResourceLimitExceeded(_)
        ));
    }

    #[tokio::test]
    async fn test_tool_dispatch_error_recorded_in_trace() {
        let engine = MockEngine::default();
        let dispatcher = SimpleDispatcher::new();

        let input = RunProgramInput {
            code: "# TOOL: unknown_tool {}".to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();
        assert_eq!(result.trace.len(), 1);
        assert!(result.trace[0].error.is_some());
        assert!(result.trace[0].result.is_none());
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
        let result = dispatcher.dispatch("missing", serde_json::json!({})).await;
        assert!(matches!(
            result.unwrap_err(),
            MontygateError::ToolNotFound(_)
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

    // === MontyEngine ===

    #[tokio::test]
    async fn test_monty_engine_input_injection_pure_computation() {
        let engine = MontyEngine::default();
        let dispatcher = SimpleDispatcher::new();

        let mut inputs = HashMap::new();
        inputs.insert("x".to_string(), serde_json::json!(10));
        inputs.insert("y".to_string(), serde_json::json!(20));

        let input = RunProgramInput {
            code: "result = x + y\nprint(result)".to_string(),
            inputs,
            type_check: true,
        };

        let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();

        assert_eq!(result.stdout.trim(), "30");
    }

    #[tokio::test]
    async fn test_monty_engine_input_injection_with_tool_call() {
        let engine = MontyEngine::default();
        let mut dispatcher = SimpleDispatcher::new();
        dispatcher.register("test.echo", |args| Ok(args));

        let mut inputs = HashMap::new();
        inputs.insert("msg".to_string(), serde_json::json!("hello"));

        let input = RunProgramInput {
            code: r#"result = tool("test.echo", {"message": msg})"#.to_string(),
            inputs,
            type_check: true,
        };

        let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();

        assert_eq!(result.trace.len(), 1);
        assert_eq!(result.trace[0].arguments["message"], "hello");
    }

    #[tokio::test]
    async fn test_monty_engine_batch_tools_execution() {
        let engine = MontyEngine::default();
        let mut dispatcher = SimpleDispatcher::new();
        dispatcher.register("test.echo", |args| Ok(args));

        let input = RunProgramInput {
            code: r#"
results = batch_tools([
    ("test.echo", {"msg": "hello"}),
    ("test.echo", {"msg": "world"}),
])
print(len(results))
"#
            .to_string(),
            inputs: HashMap::new(),
            type_check: true,
        };

        let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();

        assert_eq!(result.stdout.trim(), "2");
        assert_eq!(result.trace.len(), 2);
    }

    // === parse_batch_tool_args ===

    #[test]
    fn test_parse_batch_tool_args_valid() {
        let args = vec![MontyObject::List(vec![
            MontyObject::Tuple(vec![
                MontyObject::String("list_issues".to_string()),
                MontyObject::Dict(
                    vec![(
                        MontyObject::String("repo".to_string()),
                        MontyObject::String("foo".to_string()),
                    )]
                    .into(),
                ),
            ]),
            MontyObject::Tuple(vec![
                MontyObject::String("list_issues".to_string()),
                MontyObject::Dict(
                    vec![(
                        MontyObject::String("repo".to_string()),
                        MontyObject::String("bar".to_string()),
                    )]
                    .into(),
                ),
            ]),
        ])];

        let result = MontyEngine::parse_batch_tool_args(&args).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, "list_issues");
        assert_eq!(result[0].1["repo"], "foo");
    }

    #[test]
    fn test_parse_batch_tool_args_empty_list() {
        let args = vec![MontyObject::List(vec![])];
        let result = MontyEngine::parse_batch_tool_args(&args).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_batch_tool_args_no_args() {
        let args: Vec<MontyObject> = vec![];
        let result = MontyEngine::parse_batch_tool_args(&args);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_batch_tool_args_not_a_list() {
        let args = vec![MontyObject::String("not a list".to_string())];
        let result = MontyEngine::parse_batch_tool_args(&args);
        assert!(result.is_err());
    }
}
