use std::{fmt::Debug, future::Future, time::Duration};

/// Run a closure until the future it returns resolves to an Ok value.
///
/// Takes a closure that is expected to return a Future. The function is
/// repeatedly run and awaited, until the Result that it returns is Ok
/// or a given number of retries is met.
///
/// Optionally waits for a given duration between retries.
pub async fn do_with_retry<T, E: Debug, Fut: Future<Output = Result<T, E>>, F: Fn() -> Fut>(
    func: F,
    retries: u16,
    delay: Duration,
) -> Result<T, E> {
    let mut attempt: u16 = 1;
    loop {
        let result = func().await;

        match result {
            Ok(value) => return Ok(value),
            Err(error) => {
                if attempt >= retries {
                    tracing::error!(final_error=?error, %retries, "All attempts failed, giving up.");
                    return Err(error);
                } else if attempt == 1 {
                    tracing::warn!(error=?error, %retries, "Failed on first attempt; will keep retrying. Only the first and last error will be reported at the WARN level.");
                }

                tracing::debug!(error=?error, %attempt, %retries, "Attempt failed; retrying.");
                attempt += 1;

                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}
