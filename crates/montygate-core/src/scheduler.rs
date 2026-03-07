use crate::policy::{PolicyDecision, PolicyEngine};
use crate::retry::retry_with_backoff;
use crate::types::{ExecutionLimits, MontygateError, Result, RetryConfig};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::debug;

/// Scheduler orchestrates tool call execution with concurrency limiting,
/// timeout wrapping, retry with backoff, and policy checks.
#[derive(Debug)]
pub struct Scheduler {
    semaphore: Arc<Semaphore>,
    retry_config: RetryConfig,
    execution_limits: ExecutionLimits,
    policy: Arc<PolicyEngine>,
}

impl Scheduler {
    pub fn new(
        execution_limits: ExecutionLimits,
        retry_config: RetryConfig,
        policy: Arc<PolicyEngine>,
    ) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(execution_limits.max_concurrent)),
            retry_config,
            execution_limits,
            policy,
        }
    }

    /// Execute a tool call with policy check, concurrency limiting, timeout, and retry.
    pub async fn execute<F, Fut>(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        f: F,
    ) -> Result<serde_json::Value>
    where
        F: Fn(&str, &serde_json::Value, u32) -> Fut + Send + Sync,
        Fut: Future<Output = Result<serde_json::Value>> + Send,
    {
        // 1. Policy check
        let decision = self.policy.check(tool_name, args).await?;
        match decision {
            PolicyDecision::Allow => {}
            PolicyDecision::Deny { reason } => {
                return Err(MontygateError::PolicyViolation(reason));
            }
            PolicyDecision::RequireApproval { tool, .. } => {
                return Err(MontygateError::PolicyViolation(format!(
                    "Tool '{}' requires approval",
                    tool
                )));
            }
            PolicyDecision::RateLimitExceeded { limit, .. } => {
                return Err(MontygateError::RateLimitExceeded(limit));
            }
        }

        // 2. Acquire semaphore (concurrency limit)
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| MontygateError::Execution("Scheduler semaphore closed".to_string()))?;

        debug!("Acquired semaphore permit for tool '{}'", tool_name);

        // 3. Execute with retry + timeout
        let timeout_duration = std::time::Duration::from_millis(self.execution_limits.timeout_ms);

        let tool_name_owned = tool_name.to_string();
        let args_owned = args.clone();

        retry_with_backoff(&self.retry_config, tool_name, |attempt| {
            let name = tool_name_owned.clone();
            let args = args_owned.clone();
            let timeout = timeout_duration;
            let fut = f(&name, &args, attempt);

            async move {
                match tokio::time::timeout(timeout, fut).await {
                    Ok(result) => result,
                    Err(_) => Err(MontygateError::Timeout(format!(
                        "Tool '{}' timed out after {:?}",
                        name, timeout
                    ))),
                }
            }
        })
        .await
    }

    /// Execute a batch of tool calls in parallel, respecting concurrency limits.
    pub async fn execute_batch<F, Fut>(
        &self,
        calls: &[(String, serde_json::Value)],
        f: Arc<F>,
    ) -> Vec<Result<serde_json::Value>>
    where
        F: Fn(&str, &serde_json::Value, u32) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value>> + Send,
    {
        let handles: Vec<_> = calls
            .iter()
            .map(|(name, args)| {
                let name = name.clone();
                let args = args.clone();
                let semaphore = self.semaphore.clone();
                let policy = self.policy.clone();
                let retry_config = self.retry_config.clone();
                let execution_limits = self.execution_limits.clone();
                let f = f.clone();

                tokio::spawn(async move {
                    // Policy check
                    let decision = policy.check(&name, &args).await?;
                    match decision {
                        PolicyDecision::Allow => {}
                        PolicyDecision::Deny { reason } => {
                            return Err(MontygateError::PolicyViolation(reason));
                        }
                        PolicyDecision::RequireApproval { tool, .. } => {
                            return Err(MontygateError::PolicyViolation(format!(
                                "Tool '{}' requires approval",
                                tool
                            )));
                        }
                        PolicyDecision::RateLimitExceeded { limit, .. } => {
                            return Err(MontygateError::RateLimitExceeded(limit));
                        }
                    }

                    // Acquire semaphore
                    let _permit = semaphore.acquire().await.map_err(|_| {
                        MontygateError::Execution("Scheduler semaphore closed".to_string())
                    })?;

                    let timeout_duration =
                        std::time::Duration::from_millis(execution_limits.timeout_ms);

                    retry_with_backoff(&retry_config, &name, |attempt| {
                        let name = name.clone();
                        let args = args.clone();
                        let timeout = timeout_duration;
                        let f = f.clone();

                        async move {
                            let fut = f(&name, &args, attempt);
                            match tokio::time::timeout(timeout, fut).await {
                                Ok(result) => result,
                                Err(_) => Err(MontygateError::Timeout(format!(
                                    "Tool '{}' timed out after {:?}",
                                    name, timeout
                                ))),
                            }
                        }
                    })
                    .await
                })
            })
            .collect();

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(Err(MontygateError::Execution(format!(
                    "Task join error: {}",
                    e
                )))),
            }
        }
        results
    }

    pub fn retry_config(&self) -> &RetryConfig {
        &self.retry_config
    }

    pub fn execution_limits(&self) -> &ExecutionLimits {
        &self.execution_limits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PolicyAction, PolicyConfig, PolicyDefaults};
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    fn default_scheduler() -> Scheduler {
        Scheduler::new(
            ExecutionLimits::default(),
            RetryConfig {
                max_retries: 2,
                base_delay_ms: 1,
            },
            Arc::new(PolicyEngine::default()),
        )
    }

    #[tokio::test]
    async fn test_execute_success() {
        let scheduler = default_scheduler();

        let result = scheduler
            .execute(
                "test_tool",
                &serde_json::json!({"x": 1}),
                |_name, args, _attempt| {
                    let args = args.clone();
                    async move { Ok(args) }
                },
            )
            .await
            .unwrap();

        assert_eq!(result, serde_json::json!({"x": 1}));
    }

    #[tokio::test]
    async fn test_execute_policy_deny() {
        let policy = PolicyEngine::new(PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Deny,
            },
            rules: vec![],
        });

        let scheduler = Scheduler::new(
            ExecutionLimits::default(),
            RetryConfig::default(),
            Arc::new(policy),
        );

        let result = scheduler
            .execute("test_tool", &serde_json::json!({}), |_, _, _| async {
                Ok(serde_json::json!("should not reach"))
            })
            .await;

        assert!(matches!(
            result.unwrap_err(),
            MontygateError::PolicyViolation(_)
        ));
    }

    #[tokio::test]
    async fn test_execute_with_retry() {
        let scheduler = default_scheduler();
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = scheduler
            .execute("test_tool", &serde_json::json!({}), move |_, _, _| {
                let c = counter_clone.clone();
                async move {
                    let attempt = c.fetch_add(1, Ordering::SeqCst);
                    if attempt < 2 {
                        Err(MontygateError::Execution("connection reset".to_string()))
                    } else {
                        Ok(serde_json::json!({"ok": true}))
                    }
                }
            })
            .await
            .unwrap();

        assert_eq!(result, serde_json::json!({"ok": true}));
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        let scheduler = Scheduler::new(
            ExecutionLimits {
                timeout_ms: 10,
                max_concurrent: 5,
            },
            RetryConfig {
                max_retries: 0,
                base_delay_ms: 1,
            },
            Arc::new(PolicyEngine::default()),
        );

        let result = scheduler
            .execute("test_tool", &serde_json::json!({}), |_, _, _| async {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                Ok(serde_json::json!("should not reach"))
            })
            .await;

        // Will either be Timeout or MaxRetries wrapping Timeout
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_concurrency_limit() {
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
        for _ in 0..5 {
            let s = scheduler.clone();
            let max_c = max_concurrent.clone();
            let cur_c = current_concurrent.clone();

            handles.push(tokio::spawn(async move {
                s.execute("test_tool", &serde_json::json!({}), move |_, _, _| {
                    let max_c = max_c.clone();
                    let cur_c = cur_c.clone();
                    async move {
                        let prev = cur_c.fetch_add(1, Ordering::SeqCst);
                        max_c.fetch_max(prev + 1, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        cur_c.fetch_sub(1, Ordering::SeqCst);
                        Ok(serde_json::json!("ok"))
                    }
                })
                .await
            }));
        }

        for handle in handles {
            handle.await.unwrap().unwrap();
        }

        assert!(max_concurrent.load(Ordering::SeqCst) <= 2);
    }

    #[tokio::test]
    async fn test_execute_batch_success() {
        let scheduler = default_scheduler();

        let calls = vec![
            ("tool_a".to_string(), serde_json::json!({"id": 1})),
            ("tool_b".to_string(), serde_json::json!({"id": 2})),
        ];

        let results = scheduler
            .execute_batch(
                &calls,
                Arc::new(|_name: &str, args: &serde_json::Value, _: u32| {
                    let args = args.clone();
                    async move { Ok(args) }
                }),
            )
            .await;

        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
    }

    #[tokio::test]
    async fn test_execute_batch_partial_failure() {
        let scheduler = default_scheduler();

        let calls = vec![
            ("good_tool".to_string(), serde_json::json!({})),
            ("bad_tool".to_string(), serde_json::json!({})),
        ];

        let results = scheduler
            .execute_batch(
                &calls,
                Arc::new(|name: &str, _args: &serde_json::Value, _: u32| {
                    let name = name.to_string();
                    async move {
                        if name == "bad_tool" {
                            Err(MontygateError::Execution("permanent error".to_string()))
                        } else {
                            Ok(serde_json::json!({"ok": true}))
                        }
                    }
                }),
            )
            .await;

        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(results[1].is_err());
    }

    #[test]
    fn test_scheduler_accessors() {
        let scheduler = default_scheduler();
        assert_eq!(scheduler.retry_config().max_retries, 2);
        assert_eq!(scheduler.execution_limits().max_concurrent, 5);
    }
}
