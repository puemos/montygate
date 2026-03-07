use crate::types::{MontygateError, Result, RetryConfig};
use std::future::Future;
use tracing::{debug, warn};

/// Check if an error message indicates a retryable transient failure.
pub fn is_retryable_error(error_msg: &str) -> bool {
    let lower = error_msg.to_lowercase();
    lower.contains("connection reset")
        || lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("broken pipe")
        || lower.contains("connection refused")
        || lower.contains("stream closed")
        || lower.contains("connection closed")
        || lower.contains("eof")
        || lower.contains("temporarily unavailable")
}

/// Execute a closure with retry and exponential backoff.
///
/// Returns the result of the first successful call, or the last error
/// if all retries are exhausted.
pub async fn retry_with_backoff<F, Fut, T>(
    config: &RetryConfig,
    operation_name: &str,
    mut f: F,
) -> Result<T>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let max_attempts = config.max_retries + 1;
    let base_delay = std::time::Duration::from_millis(config.base_delay_ms);

    for attempt in 0..max_attempts {
        if attempt > 0 {
            let delay = base_delay * 2u32.saturating_pow(attempt - 1);
            debug!(
                "Retrying '{}' (attempt {}/{}), delay {:?}",
                operation_name,
                attempt + 1,
                max_attempts,
                delay
            );
            tokio::time::sleep(delay).await;
        }

        match f(attempt).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                let err_msg = e.to_string();
                if is_retryable_error(&err_msg) && attempt + 1 < max_attempts {
                    warn!(
                        "Retryable error in '{}': {}",
                        operation_name, err_msg
                    );
                    continue;
                }
                if attempt + 1 >= max_attempts {
                    return Err(MontygateError::MaxRetries(format!(
                        "'{}' failed after {} attempts: {}",
                        operation_name, max_attempts, err_msg
                    )));
                }
                return Err(e);
            }
        }
    }

    Err(MontygateError::MaxRetries(format!(
        "'{}' failed after {} attempts",
        operation_name, max_attempts
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    // === is_retryable_error ===

    #[test]
    fn test_is_retryable_error_connection_reset() {
        assert!(is_retryable_error("connection reset by peer"));
        assert!(is_retryable_error("Connection Reset"));
    }

    #[test]
    fn test_is_retryable_error_timeout() {
        assert!(is_retryable_error("request timeout"));
        assert!(is_retryable_error("connection timed out"));
        assert!(is_retryable_error("operation Timeout"));
    }

    #[test]
    fn test_is_retryable_error_broken_pipe() {
        assert!(is_retryable_error("broken pipe"));
        assert!(is_retryable_error("Broken Pipe error"));
    }

    #[test]
    fn test_is_retryable_error_connection_refused() {
        assert!(is_retryable_error("connection refused"));
    }

    #[test]
    fn test_is_retryable_error_stream_closed() {
        assert!(is_retryable_error("stream closed unexpectedly"));
        assert!(is_retryable_error("connection closed"));
    }

    #[test]
    fn test_is_retryable_error_eof() {
        assert!(is_retryable_error("unexpected eof"));
    }

    #[test]
    fn test_is_retryable_error_temporarily_unavailable() {
        assert!(is_retryable_error("resource temporarily unavailable"));
    }

    #[test]
    fn test_is_retryable_error_non_retryable() {
        assert!(!is_retryable_error("invalid argument"));
        assert!(!is_retryable_error("permission denied"));
        assert!(!is_retryable_error("not found"));
        assert!(!is_retryable_error("bad request"));
    }

    // === retry_with_backoff ===

    #[tokio::test]
    async fn test_retry_succeeds_first_try() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay_ms: 1,
        };

        let result = retry_with_backoff(&config, "test", |_| async {
            Ok::<_, MontygateError>(42)
        })
        .await;

        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay_ms: 1,
        };

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = retry_with_backoff(&config, "test", move |_| {
            let c = counter_clone.clone();
            async move {
                let attempt = c.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    Err(MontygateError::Execution("connection reset".to_string()))
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let config = RetryConfig {
            max_retries: 2,
            base_delay_ms: 1,
        };

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = retry_with_backoff(&config, "test", move |_| {
            let c = counter_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(MontygateError::Execution("connection reset".to_string()))
            }
        })
        .await;

        assert!(matches!(result.unwrap_err(), MontygateError::MaxRetries(_)));
        assert_eq!(counter.load(Ordering::SeqCst), 3); // 1 initial + 2 retries
    }

    #[tokio::test]
    async fn test_retry_non_retryable_error_fails_immediately() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay_ms: 1,
        };

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = retry_with_backoff(&config, "test", move |_| {
            let c = counter_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(MontygateError::Execution("invalid argument".to_string()))
            }
        })
        .await;

        assert!(result.is_err());
        // Non-retryable error should not retry
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_zero_retries() {
        let config = RetryConfig {
            max_retries: 0,
            base_delay_ms: 1,
        };

        let result = retry_with_backoff(&config, "test", |_| async {
            Err::<i32, _>(MontygateError::Execution("connection reset".to_string()))
        })
        .await;

        assert!(matches!(result.unwrap_err(), MontygateError::MaxRetries(_)));
    }
}
