use std::future::Future;
use std::time::Duration;

use ragent_types::config::RetryConfig;
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
#[error("max retries ({max_retries}) exceeded: {last_error}")]
pub struct RetryExhausted {
    pub max_retries: u32,
    pub last_error: String,
}

/// Execute an async function with exponential backoff retry.
pub async fn with_retry<F, Fut, T>(
    config: &RetryConfig,
    mut operation: F,
) -> Result<T, RetryExhausted>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, anyhow::Error>>,
{
    if !config.enabled {
        return operation().await.map_err(|e| RetryExhausted {
            max_retries: 0,
            last_error: e.to_string(),
        });
    }

    let mut last_error = String::new();

    for attempt in 0..=config.max_retries {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                last_error = e.to_string();
                if attempt < config.max_retries {
                    let delay = calculate_delay(config, attempt);
                    warn!(
                        attempt = attempt + 1,
                        max_retries = config.max_retries,
                        delay_ms = delay.as_millis() as u64,
                        error = %e,
                        "retrying after error"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    Err(RetryExhausted {
        max_retries: config.max_retries,
        last_error,
    })
}

fn calculate_delay(config: &RetryConfig, attempt: u32) -> Duration {
    let base = config.initial_delay_ms as f64;
    let delay = base * 2.0_f64.powi(attempt as i32);
    let capped = delay.min(config.max_delay_ms as f64) as u64;
    Duration::from_millis(capped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn fast_retry_config(max_retries: u32) -> RetryConfig {
        RetryConfig {
            enabled: true,
            max_retries,
            initial_delay_ms: 1,
            max_delay_ms: 10,
        }
    }

    #[tokio::test]
    async fn retry_succeeds_immediately() {
        let config = fast_retry_config(3);
        let result = with_retry(&config, || async { Ok::<_, anyhow::Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn retry_succeeds_after_failures() {
        let config = fast_retry_config(3);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = with_retry(&config, move || {
            let attempts = attempts_clone.clone();
            async move {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    anyhow::bail!("transient error")
                }
                Ok(42)
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_exhausted() {
        let config = fast_retry_config(2);
        let result = with_retry(&config, || async {
            Err::<i32, _>(anyhow::anyhow!("permanent error"))
        })
        .await;

        let err = result.unwrap_err();
        assert_eq!(err.max_retries, 2);
        assert!(err.last_error.contains("permanent error"));
    }

    #[tokio::test]
    async fn retry_disabled_no_retry() {
        let config = RetryConfig {
            enabled: false,
            ..fast_retry_config(3)
        };
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = with_retry(&config, move || {
            let attempts = attempts_clone.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(anyhow::anyhow!("error"))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn delay_calculation() {
        let config = RetryConfig {
            enabled: true,
            max_retries: 5,
            initial_delay_ms: 100,
            max_delay_ms: 5000,
        };
        assert_eq!(calculate_delay(&config, 0), Duration::from_millis(100));
        assert_eq!(calculate_delay(&config, 1), Duration::from_millis(200));
        assert_eq!(calculate_delay(&config, 2), Duration::from_millis(400));
        assert_eq!(calculate_delay(&config, 3), Duration::from_millis(800));
        assert_eq!(calculate_delay(&config, 10), Duration::from_millis(5000)); // capped
    }
}
