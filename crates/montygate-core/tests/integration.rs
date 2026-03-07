use montygate_core::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

// =============================================================================
// Registry integration tests
// =============================================================================

#[test]
fn test_registry_register_search_catalog() {
    let registry = ToolRegistry::new();

    registry
        .register_tools(vec![
            ToolDefinition {
                name: "lookup_order".to_string(),
                description: Some("Look up order details by order ID".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "order_id": {"type": "string"}
                    },
                    "required": ["order_id"]
                }),
                output_schema: None,
            },
            ToolDefinition {
                name: "create_ticket".to_string(),
                description: Some("Create a support ticket".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "subject": {"type": "string"},
                        "body": {"type": "string"}
                    },
                    "required": ["subject", "body"]
                }),
                output_schema: None,
            },
            ToolDefinition {
                name: "send_email".to_string(),
                description: Some("Send an email to a recipient".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "string"},
                        "subject": {"type": "string"},
                        "body": {"type": "string"}
                    },
                    "required": ["to", "subject", "body"]
                }),
                output_schema: None,
            },
        ])
        .unwrap();

    assert_eq!(registry.tool_count(), 3);

    // Search by name
    let results = registry.search_tools("order", 5);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "lookup_order");

    // Search by description
    let results = registry.search_tools("ticket", 5);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "create_ticket");

    // Search matching multiple (both descriptions mention what they do)
    let results = registry.search_tools("order", 5);
    assert_eq!(results.len(), 1); // Only lookup_order matches

    // Catalog generation
    let catalog = registry.tool_catalog();
    assert!(catalog.contains("lookup_order("));
    assert!(catalog.contains("create_ticket("));
    assert!(catalog.contains("send_email("));
    assert!(catalog.contains("Look up order details"));
}

// =============================================================================
// Scheduler integration tests
// =============================================================================

#[tokio::test]
async fn test_scheduler_concurrency_limiting() {
    let scheduler = Arc::new(Scheduler::new(
        ExecutionLimits {
            timeout_ms: 30_000,
            max_concurrent: 2,
        },
        RetryConfig {
            max_retries: 0,
            base_delay_ms: 1,
        },
        Arc::new(PolicyEngine::default()),
    ));

    let max_concurrent = Arc::new(AtomicUsize::new(0));
    let current_concurrent = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for i in 0..5 {
        let s = scheduler.clone();
        let max_c = max_concurrent.clone();
        let cur_c = current_concurrent.clone();

        handles.push(tokio::spawn(async move {
            s.execute(
                &format!("tool_{}", i),
                &serde_json::json!({}),
                move |_, _, _| {
                    let max_c = max_c.clone();
                    let cur_c = cur_c.clone();
                    async move {
                        let prev = cur_c.fetch_add(1, Ordering::SeqCst);
                        max_c.fetch_max(prev + 1, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        cur_c.fetch_sub(1, Ordering::SeqCst);
                        Ok(serde_json::json!({"ok": true}))
                    }
                },
            )
            .await
        }));
    }

    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    // With max_concurrent=2, we should never exceed 2 concurrent calls
    assert!(
        max_concurrent.load(Ordering::SeqCst) <= 2,
        "Max concurrent was {}, expected <= 2",
        max_concurrent.load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn test_scheduler_retry_then_succeed() {
    let scheduler = Scheduler::new(
        ExecutionLimits::default(),
        RetryConfig {
            max_retries: 3,
            base_delay_ms: 1,
        },
        Arc::new(PolicyEngine::default()),
    );

    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let result = scheduler
        .execute("flaky_tool", &serde_json::json!({}), move |_, _, _| {
            let c = counter_clone.clone();
            async move {
                let attempt = c.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    // First two attempts fail with retryable error
                    Err(MontygateError::Execution("connection reset".to_string()))
                } else {
                    // Third attempt succeeds
                    Ok(serde_json::json!({"status": "success", "attempt": attempt}))
                }
            }
        })
        .await
        .unwrap();

    assert_eq!(result["status"], "success");
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn test_scheduler_timeout() {
    let scheduler = Scheduler::new(
        ExecutionLimits {
            timeout_ms: 50,
            max_concurrent: 5,
        },
        RetryConfig {
            max_retries: 0,
            base_delay_ms: 1,
        },
        Arc::new(PolicyEngine::default()),
    );

    let result = scheduler
        .execute("slow_tool", &serde_json::json!({}), |_, _, _| async {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            Ok(serde_json::json!("should not reach"))
        })
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("timed out") || err.contains("Timeout") || err.contains("retries"));
}

// =============================================================================
// Policy + Scheduler integration tests
// =============================================================================

#[tokio::test]
async fn test_scheduler_policy_deny() {
    let policy = PolicyEngine::new(PolicyConfig {
        defaults: PolicyDefaults::default(),
        rules: vec![PolicyRule {
            match_pattern: "dangerous_tool".to_string(),
            action: PolicyAction::Deny,
            rate_limit: None,
        }],
    });

    let scheduler = Scheduler::new(
        ExecutionLimits::default(),
        RetryConfig::default(),
        Arc::new(policy),
    );

    let result = scheduler
        .execute("dangerous_tool", &serde_json::json!({}), |_, _, _| async {
            Ok(serde_json::json!("should not reach"))
        })
        .await;

    assert!(matches!(
        result.unwrap_err(),
        MontygateError::PolicyViolation(_)
    ));
}

#[tokio::test]
async fn test_scheduler_policy_rate_limit() {
    let policy = PolicyEngine::new(PolicyConfig {
        defaults: PolicyDefaults::default(),
        rules: vec![PolicyRule {
            match_pattern: "limited_tool".to_string(),
            action: PolicyAction::Allow,
            rate_limit: Some("1/min".to_string()),
        }],
    });

    let scheduler = Scheduler::new(
        ExecutionLimits::default(),
        RetryConfig {
            max_retries: 0,
            base_delay_ms: 1,
        },
        Arc::new(policy),
    );

    // First call succeeds
    let result = scheduler
        .execute("limited_tool", &serde_json::json!({}), |_, _, _| async {
            Ok(serde_json::json!({"ok": true}))
        })
        .await;
    assert!(result.is_ok());

    // Second call hits rate limit
    let result = scheduler
        .execute("limited_tool", &serde_json::json!({}), |_, _, _| async {
            Ok(serde_json::json!({"ok": true}))
        })
        .await;
    assert!(matches!(
        result.unwrap_err(),
        MontygateError::RateLimitExceeded(_)
    ));
}

// =============================================================================
// Engine + full flow integration tests
// =============================================================================

#[tokio::test]
async fn test_monty_engine_multi_tool_script() {
    let engine = MontyEngine::default();
    let mut dispatcher = SimpleDispatcher::new();

    dispatcher.register("lookup_order", |args| {
        let order_id = args["order_id"].as_str().unwrap_or("unknown");
        Ok(serde_json::json!({
            "id": order_id,
            "status": "shipped",
            "email": "customer@example.com",
            "details": "Order details here"
        }))
    });

    dispatcher.register("create_ticket", |args| {
        Ok(serde_json::json!({
            "ticket_id": "TK-001",
            "subject": args["subject"],
            "status": "open"
        }))
    });

    dispatcher.register("send_email", |_args| Ok(serde_json::json!({"sent": true})));

    let input = RunProgramInput {
        code: r#"
order = tool("lookup_order", {"order_id": "123"})
ticket = tool("create_ticket", {"subject": "Late order " + order["id"], "body": order["details"]})
tool("send_email", {"to": order["email"], "subject": ticket["subject"], "body": "Ticket created"})
ticket
"#
        .to_string(),
        inputs: HashMap::new(),
        type_check: true,
    };

    let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();

    // Verify the script executed all 3 tool calls
    assert_eq!(result.trace.len(), 3);
    assert_eq!(result.trace[0].tool, "lookup_order");
    assert_eq!(result.trace[1].tool, "create_ticket");
    assert_eq!(result.trace[2].tool, "send_email");

    // Verify the final result is the ticket
    assert_eq!(result.output["ticket_id"], "TK-001");
    assert_eq!(result.output["status"], "open");

    // Verify tool call arguments were correctly passed
    assert_eq!(result.trace[0].arguments["order_id"], "123");
    assert!(
        result.trace[1].arguments["subject"]
            .as_str()
            .unwrap()
            .contains("Late order 123")
    );
}

#[tokio::test]
async fn test_monty_engine_with_input_injection() {
    let engine = MontyEngine::default();
    let mut dispatcher = SimpleDispatcher::new();

    dispatcher.register("greet", |args| {
        let name = args["name"].as_str().unwrap_or("world");
        Ok(serde_json::json!({"greeting": format!("Hello, {}!", name)}))
    });

    let mut inputs = HashMap::new();
    inputs.insert("user_name".to_string(), serde_json::json!("Alice"));

    let input = RunProgramInput {
        code: r#"tool("greet", {"name": user_name})"#.to_string(),
        inputs,
        type_check: true,
    };

    let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();

    assert_eq!(result.trace.len(), 1);
    assert_eq!(result.output["greeting"], "Hello, Alice!");
}

#[tokio::test]
async fn test_monty_engine_batch_parallel_dispatch() {
    let engine = MontyEngine::default();
    let mut dispatcher = SimpleDispatcher::new();

    dispatcher.register("fetch_data", |args| {
        let id = args["id"].as_i64().unwrap_or(0);
        Ok(serde_json::json!({"id": id, "data": format!("data_{}", id)}))
    });

    let input = RunProgramInput {
        code: r#"
results = batch_tools([
    ("fetch_data", {"id": 1}),
    ("fetch_data", {"id": 2}),
    ("fetch_data", {"id": 3}),
])
len(results)
"#
        .to_string(),
        inputs: HashMap::new(),
        type_check: true,
    };

    let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();

    // All 3 batch calls should be in the trace
    assert_eq!(result.trace.len(), 3);
    assert_eq!(result.output, serde_json::json!(3));
}

// =============================================================================
// Observability integration tests
// =============================================================================

#[test]
fn test_execution_tracer_full_flow() {
    let tracer = ExecutionTracer::new();

    // Record a successful call
    tracer.record_success(
        "lookup_order",
        serde_json::json!({"order_id": "123"}),
        serde_json::json!({"id": "123", "status": "shipped"}),
        42,
        0,
    );

    // Record a failed call that was retried
    tracer.record_error(
        "create_ticket",
        serde_json::json!({"subject": "test"}),
        "connection reset".to_string(),
        5000,
        2,
    );

    // Record another success
    tracer.record_success(
        "send_email",
        serde_json::json!({"to": "a@b.com"}),
        serde_json::json!({"sent": true}),
        100,
        0,
    );

    assert_eq!(tracer.len(), 3);

    let entries = tracer.entries();

    // First entry: success
    assert_eq!(entries[0].tool_name, "lookup_order");
    assert!(entries[0].output.is_some());
    assert!(entries[0].error.is_none());
    assert_eq!(entries[0].retries, 0);

    // Second entry: error with retries
    assert_eq!(entries[1].tool_name, "create_ticket");
    assert!(entries[1].output.is_none());
    assert_eq!(entries[1].error.as_deref(), Some("connection reset"));
    assert_eq!(entries[1].retries, 2);

    // Third entry: success
    assert_eq!(entries[2].tool_name, "send_email");
    assert!(entries[2].output.is_some());
}

// =============================================================================
// Token savings demonstration
// =============================================================================

#[tokio::test]
async fn test_token_savings_single_script_vs_multiple_calls() {
    // This test demonstrates the core value proposition:
    // One script with 3 tool calls returns 1 result,
    // vs 3 separate tool calls returning 3 results.

    let engine = MontyEngine::default();
    let mut dispatcher = SimpleDispatcher::new();

    dispatcher.register("lookup_order", |_| {
        Ok(serde_json::json!({"id": "ORD-123", "email": "user@test.com", "details": "Widget x5"}))
    });
    dispatcher.register("create_ticket", |args| {
        Ok(serde_json::json!({"ticket_id": "TK-001", "subject": args["subject"]}))
    });
    dispatcher.register("send_email", |_| {
        Ok(serde_json::json!({"sent": true, "message_id": "MSG-001"}))
    });

    // WITH montygate: 1 script, 1 result back to LLM
    let input = RunProgramInput {
        code: r#"
order = tool("lookup_order", {"order_id": "123"})
ticket = tool("create_ticket", {"subject": "Late: " + order["id"], "body": order["details"]})
tool("send_email", {"to": order["email"], "subject": ticket["subject"], "body": "Created"})
ticket
"#
        .to_string(),
        inputs: HashMap::new(),
        type_check: true,
    };

    let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();

    // Only the final result (ticket) goes back to the LLM
    assert_eq!(result.output["ticket_id"], "TK-001");

    // But all 3 calls were made internally
    assert_eq!(result.trace.len(), 3);
    assert_eq!(result.stats.external_calls, 3);

    // The key insight: the LLM only sees 1 result (the ticket),
    // not all 3 intermediate results. This saves tokens.
}

// =============================================================================
// Retry integration test
// =============================================================================

#[tokio::test]
async fn test_retry_with_backoff_integration() {
    let config = RetryConfig {
        max_retries: 3,
        base_delay_ms: 1,
    };

    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let result = retry_with_backoff(&config, "integration_test", move |_attempt| {
        let c = counter_clone.clone();
        async move {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(MontygateError::Execution("connection reset".to_string()))
            } else {
                Ok(serde_json::json!({"recovered": true}))
            }
        }
    })
    .await
    .unwrap();

    assert_eq!(result, serde_json::json!({"recovered": true}));
    assert_eq!(counter.load(Ordering::SeqCst), 3); // 1 initial + 2 retries
}

// =============================================================================
// Scheduler batch integration tests
// =============================================================================

#[tokio::test]
async fn test_scheduler_batch_with_mixed_results() {
    let scheduler = Scheduler::new(
        ExecutionLimits {
            timeout_ms: 30_000,
            max_concurrent: 3,
        },
        RetryConfig {
            max_retries: 0,
            base_delay_ms: 1,
        },
        Arc::new(PolicyEngine::default()),
    );

    let calls = vec![
        ("good_tool".to_string(), serde_json::json!({"id": 1})),
        ("bad_tool".to_string(), serde_json::json!({"id": 2})),
        ("good_tool".to_string(), serde_json::json!({"id": 3})),
    ];

    let results = scheduler
        .execute_batch(
            &calls,
            Arc::new(|name: &str, args: &serde_json::Value, _: u32| {
                let name = name.to_string();
                let args = args.clone();
                async move {
                    if name == "bad_tool" {
                        Err(MontygateError::Execution("permanent failure".to_string()))
                    } else {
                        Ok(args)
                    }
                }
            }),
        )
        .await;

    assert_eq!(results.len(), 3);
    assert!(results[0].is_ok());
    assert!(results[1].is_err());
    assert!(results[2].is_ok());
}

// =============================================================================
// Full pipeline: Registry + Engine + Scheduler
// =============================================================================

#[tokio::test]
async fn test_full_pipeline_registry_engine() {
    // 1. Set up registry
    let registry = ToolRegistry::new();
    registry
        .register_tools(vec![
            ToolDefinition {
                name: "add".to_string(),
                description: Some("Add two numbers".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "a": {"type": "number"},
                        "b": {"type": "number"}
                    },
                    "required": ["a", "b"]
                }),
                output_schema: None,
            },
            ToolDefinition {
                name: "multiply".to_string(),
                description: Some("Multiply two numbers".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "a": {"type": "number"},
                        "b": {"type": "number"}
                    },
                    "required": ["a", "b"]
                }),
                output_schema: None,
            },
        ])
        .unwrap();

    // 2. Verify tools are searchable
    let math_tools = registry.search_tools("number", 5);
    assert_eq!(math_tools.len(), 2);

    // 3. Set up dispatcher that uses registry
    let registry = Arc::new(registry);
    let registry_for_dispatch = registry.clone();
    let mut dispatcher = SimpleDispatcher::new();

    dispatcher.register("add", |args| {
        let a = args["a"].as_f64().unwrap_or(0.0);
        let b = args["b"].as_f64().unwrap_or(0.0);
        Ok(serde_json::json!(a + b))
    });

    dispatcher.register("multiply", |args| {
        let a = args["a"].as_f64().unwrap_or(0.0);
        let b = args["b"].as_f64().unwrap_or(0.0);
        Ok(serde_json::json!(a * b))
    });

    // 4. Execute a script that uses both tools
    let engine = MontyEngine::default();
    let input = RunProgramInput {
        code: r#"
sum_result = tool("add", {"a": 3, "b": 4})
product = tool("multiply", {"a": sum_result, "b": 5})
product
"#
        .to_string(),
        inputs: HashMap::new(),
        type_check: true,
    };

    let result = engine.execute(input, Arc::new(dispatcher)).await.unwrap();

    // 3 + 4 = 7, 7 * 5 = 35
    assert_eq!(result.output, serde_json::json!(35.0));
    assert_eq!(result.trace.len(), 2);

    // 5. Verify catalog includes both tools
    let catalog = registry_for_dispatch.tool_catalog();
    assert!(catalog.contains("add("));
    assert!(catalog.contains("multiply("));
}

// =============================================================================
// Centralized prompt & schema integration tests
// =============================================================================

#[test]
fn test_system_prompt_is_consistent_across_registry_instances() {
    let r1 = ToolRegistry::new();
    let r2 = ToolRegistry::new();
    r2.register_tool(ToolDefinition {
        name: "some_tool".to_string(),
        description: Some("A tool".to_string()),
        input_schema: serde_json::json!({"type": "object"}),
        output_schema: None,
    })
    .unwrap();

    // System prompt is tool-independent — same regardless of registered tools
    assert_eq!(r1.system_prompt(), r2.system_prompt());
}

#[test]
fn test_input_schemas_are_valid_json_schema() {
    let registry = ToolRegistry::new();

    // Execute schema
    let exec_schema = registry.execute_tool_input_schema();
    assert_eq!(exec_schema["type"], "object");
    let exec_props = exec_schema["properties"].as_object().unwrap();
    assert_eq!(exec_props.len(), 2, "execute schema should have exactly 2 properties");
    // Every property must have a type
    for (key, prop) in exec_props {
        assert!(
            prop.get("type").is_some(),
            "execute property '{}' is missing 'type'",
            key
        );
    }

    // Search schema
    let search_schema = registry.search_tool_input_schema();
    assert_eq!(search_schema["type"], "object");
    let search_props = search_schema["properties"].as_object().unwrap();
    assert_eq!(search_props.len(), 2, "search schema should have exactly 2 properties");
    for (key, prop) in search_props {
        assert!(
            prop.get("type").is_some(),
            "search property '{}' is missing 'type'",
            key
        );
    }
}

#[test]
fn test_execute_description_includes_registered_tools() {
    let registry = ToolRegistry::new();
    registry
        .register_tools(vec![
            ToolDefinition {
                name: "alpha".to_string(),
                description: Some("First tool".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"x": {"type": "string"}},
                    "required": ["x"]
                }),
                output_schema: None,
            },
            ToolDefinition {
                name: "beta".to_string(),
                description: Some("Second tool".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"y": {"type": "number"}},
                    "required": ["y"]
                }),
                output_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {"result": {"type": "number"}}
                })),
            },
        ])
        .unwrap();

    let desc = registry.execute_tool_description();

    // Must contain both tools with their signatures
    assert!(desc.contains("alpha(x: string)"), "Missing alpha signature");
    assert!(desc.contains("beta(y: number)"), "Missing beta signature");
    assert!(desc.contains("First tool"));
    assert!(desc.contains("Second tool"));
    // Output schema annotation for beta
    assert!(desc.contains("->"));

    // Must still contain instructions
    assert!(desc.contains("FRESH sandbox"));
    assert!(desc.contains("batch_tools"));
}
