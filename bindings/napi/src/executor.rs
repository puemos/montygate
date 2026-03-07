use crate::types::*;
use dashmap::DashMap;
use montygate_core::engine::{EngineManager, ToolDispatcher};
use montygate_core::observability::ExecutionTracer;
use montygate_core::policy::PolicyEngine;
use montygate_core::registry::ToolRegistry;
use montygate_core::scheduler::Scheduler;
use montygate_core::types::{
    PolicyAction, PolicyConfig, PolicyDefaults, PolicyRule, RunProgramInput,
};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeFunction;
use napi_derive::napi;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

type RawToolTsfn = ThreadsafeFunction<
    serde_json::Value,
    Promise<serde_json::Value>,
    serde_json::Value,
    Status,
    false,
>;

type ToolTsfn = Arc<RawToolTsfn>;

/// Dispatcher that calls back into JS via ThreadsafeFunction when a tool is invoked.
struct NapiDispatcher {
    run_functions: Arc<DashMap<String, ToolTsfn>>,
    scheduler: Arc<Scheduler>,
}

impl std::fmt::Debug for NapiDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tools: Vec<String> = self
            .run_functions
            .iter()
            .map(|e| e.key().clone())
            .collect();
        f.debug_struct("NapiDispatcher")
            .field("tools", &tools)
            .finish()
    }
}

#[async_trait::async_trait]
impl ToolDispatcher for NapiDispatcher {
    async fn dispatch(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> montygate_core::Result<serde_json::Value> {
        let tsfn = self
            .run_functions
            .get(tool_name)
            .ok_or_else(|| montygate_core::MontygateError::ToolNotFound(tool_name.to_string()))?;

        let tsfn = tsfn.value().clone();
        let tool_name_owned = tool_name.to_string();
        let tool_name_for_closure = tool_name_owned.clone();

        self.scheduler
            .execute(&tool_name_owned, &args, move |_name, args, _attempt| {
                let tsfn = tsfn.clone();
                let tool_name = tool_name_for_closure.clone();
                let args = args.clone();
                async move {
                    let promise = tsfn.call_async(args).await.map_err(|e| {
                        montygate_core::MontygateError::Execution(format!(
                            "JS callback error for '{}': {}",
                            tool_name, e
                        ))
                    })?;

                    promise.await.map_err(|e| {
                        montygate_core::MontygateError::Execution(format!(
                            "JS promise rejected for '{}': {}",
                            tool_name, e
                        ))
                    })
                }
            })
            .await
    }
}

/// The main NAPI class exposed to Node.js.
///
/// Usage from JS/TS:
/// ```js
/// const engine = new NativeEngine({ retry: { maxRetries: 3 }, limits: { timeoutMs: 30000 } });
/// engine.registerTool({ name: "lookup", description: "...", inputSchema: {} }, async (args) => { ... });
/// const result = await engine.execute("order = tool('lookup', order_id='123')\norder", {});
/// ```
#[napi]
pub struct NativeEngine {
    registry: ToolRegistry,
    engine_manager: EngineManager,
    scheduler: Arc<Scheduler>,
    tracer: ExecutionTracer,
    run_functions: Arc<DashMap<String, ToolTsfn>>,
}

#[napi]
impl NativeEngine {
    #[napi(constructor)]
    pub fn new(config: Option<NapiConfig>) -> Self {
        let config = config.unwrap_or(NapiConfig {
            retry: None,
            limits: None,
            resource_limits: None,
            policy: None,
        });

        let retry_config = config
            .retry
            .as_ref()
            .map(|r| r.to_core())
            .unwrap_or_default();

        let execution_limits = config
            .limits
            .as_ref()
            .map(|l| l.to_core())
            .unwrap_or_default();

        let resource_limits = config
            .resource_limits
            .as_ref()
            .map(|r| r.to_core())
            .unwrap_or_default();

        let policy_engine = build_policy_engine(config.policy.as_ref());

        let scheduler = Arc::new(Scheduler::new(
            execution_limits,
            retry_config,
            Arc::new(policy_engine),
        ));

        Self {
            registry: ToolRegistry::new(),
            engine_manager: EngineManager::with_monty(resource_limits),
            scheduler,
            tracer: ExecutionTracer::new(),
            run_functions: Arc::new(DashMap::new()),
        }
    }

    /// Register a tool with its definition and JS callback function.
    ///
    /// The callback receives a JSON value (the tool arguments) and must return
    /// a Promise that resolves to a JSON value (the tool result).
    #[napi(
        ts_args_type = "definition: NapiToolDefinition, run: (args: any) => Promise<any>"
    )]
    pub fn register_tool(
        &self,
        definition: NapiToolDefinition,
        run: Function<'_, serde_json::Value, Promise<serde_json::Value>>,
    ) -> Result<()> {
        let core_def = montygate_core::types::ToolDefinition::from(&definition);
        self.registry
            .register_tool(core_def)
            .map_err(|e| Error::from_reason(e.to_string()))?;

        let tsfn: ToolTsfn = Arc::new(run.build_threadsafe_function().build()?);

        self.run_functions.insert(definition.name.clone(), tsfn);

        debug!("Registered tool '{}' with JS callback", definition.name);
        Ok(())
    }

    /// Execute a Python script with access to all registered tools.
    ///
    /// When the script calls `tool('name', ...)`, the corresponding JS callback is invoked.
    /// Only the final expression value is returned.
    #[napi]
    pub async fn execute(
        &self,
        code: String,
        inputs: Option<serde_json::Value>,
    ) -> Result<NapiExecutionResult> {
        let input_map: HashMap<String, serde_json::Value> = match inputs {
            Some(serde_json::Value::Object(map)) => map.into_iter().collect(),
            Some(_) => {
                return Err(Error::from_reason("inputs must be an object or undefined"));
            }
            None => HashMap::new(),
        };

        let program_input = RunProgramInput {
            code,
            inputs: input_map,
            type_check: true,
        };

        let dispatcher = Arc::new(NapiDispatcher {
            run_functions: self.run_functions.clone(),
            scheduler: self.scheduler.clone(),
        });

        let result = self
            .engine_manager
            .execute(program_input, dispatcher)
            .await
            .map_err(|e| Error::from_reason(e.to_string()))?;

        // Record traces
        for call in &result.trace {
            match (&call.result, &call.error) {
                (Some(output), _) => {
                    self.tracer.record_success(
                        &call.tool,
                        call.arguments.clone(),
                        output.clone(),
                        call.duration_ms,
                        call.retries,
                    );
                }
                (_, Some(err)) => {
                    self.tracer.record_error(
                        &call.tool,
                        call.arguments.clone(),
                        err.clone(),
                        call.duration_ms,
                        call.retries,
                    );
                }
                _ => {}
            }
        }

        Ok(NapiExecutionResult::from(&result))
    }

    /// Search registered tools by keyword query.
    #[napi]
    pub fn search(&self, query: String, top_k: Option<u32>) -> Vec<NapiSearchResult> {
        let k = top_k.unwrap_or(5) as usize;
        self.registry
            .search_tools(&query, k)
            .iter()
            .map(NapiSearchResult::from)
            .collect()
    }

    /// Get a formatted catalog of all registered tools (for LLM descriptions).
    #[napi]
    pub fn get_tool_catalog(&self) -> String {
        self.registry.tool_catalog()
    }

    /// Get the number of registered tools.
    #[napi]
    pub fn tool_count(&self) -> u32 {
        self.registry.tool_count() as u32
    }

    /// Get all trace entries recorded so far.
    #[napi]
    pub fn get_traces(&self) -> Vec<NapiTraceEntry> {
        self.tracer
            .entries()
            .iter()
            .map(|entry| NapiTraceEntry {
                tool_name: entry.tool_name.clone(),
                input: entry.input.clone(),
                output: entry.output.clone(),
                error: entry.error.clone(),
                duration_ms: entry.duration_ms as u32,
                retries: entry.retries,
            })
            .collect()
    }

    /// Clear all trace entries.
    #[napi]
    pub fn clear_traces(&self) {
        self.tracer.clear();
    }
}

fn build_policy_engine(config: Option<&NapiPolicyConfig>) -> PolicyEngine {
    let Some(config) = config else {
        return PolicyEngine::default();
    };

    let default_action = match config.default_action.as_deref() {
        Some("deny") => PolicyAction::Deny,
        Some("require_approval") => PolicyAction::RequireApproval,
        _ => PolicyAction::Allow,
    };

    let rules = config
        .rules
        .as_ref()
        .map(|rules| {
            rules
                .iter()
                .map(|r| {
                    let action = match r.action.as_str() {
                        "deny" => PolicyAction::Deny,
                        "require_approval" => PolicyAction::RequireApproval,
                        _ => PolicyAction::Allow,
                    };
                    PolicyRule {
                        match_pattern: r.match_pattern.clone(),
                        action,
                        rate_limit: r.rate_limit.clone(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    PolicyEngine::new(PolicyConfig {
        defaults: PolicyDefaults {
            action: default_action,
        },
        rules,
    })
}
