use anyhow::{Context, Result};
use std::{thread::sleep, time::Duration};
use tracing::error;

/// Retry an operation with exponential backoff
///
/// # Arguments
/// * `max_retries` - Maximum number of retry attempts
/// * `delay_secs` - Delay in seconds between retries
/// * `f` - The operation to retry
///
/// # Returns
/// * `Ok(T)` - The result of the successful operation
/// * `Err(anyhow::Error)` - The error from the last failed attempt
///
/// # Example
/// ```
/// let result = retry_with_backoff(3, 5, || {
///     // Your operation here
///     Ok(42)
/// })?;
/// ```
pub fn retry_with_backoff<F, T>(max_retries: u32, delay_secs: u64, mut f: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let mut attempts = 0;

    loop {
        match f() {
            Ok(result) => return Ok(result),
            Err(err) => {
                attempts += 1;

                if attempts >= max_retries {
                    return Err(err)
                        .context(format!("Operation failed after {} attempts", max_retries));
                }

                error!(
                    ?err,
                    attempt = attempts,
                    max_retries,
                    "Operation failed, retrying in {}s...",
                    delay_secs
                );

                sleep(Duration::from_secs(delay_secs));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_retry_success_on_first_attempt() {
        let result = retry_with_backoff(3, 1, || Ok(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_retry_success_after_failures() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = retry_with_backoff(3, 0, move || {
            let count = counter_clone.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                anyhow::bail!("Temporary failure")
            } else {
                Ok(42)
            }
        });

        assert_eq!(result.unwrap(), 42);
        assert_eq!(counter.load(Ordering::SeqCst), 3); // Failed twice, succeeded on third
    }

    #[test]
    fn test_retry_exhausts_attempts() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result: Result<i32> = retry_with_backoff(3, 0, move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("Always fails")
        });

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 3); // Tried exactly 3 times
    }
}
