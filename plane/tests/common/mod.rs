use self::test_env::TestEnvironment;
use futures_util::Future;
use std::{panic::AssertUnwindSafe, time::Duration};
use tokio::time::timeout;

pub mod async_drop;
pub mod docker;
pub mod resources;
pub mod test_env;
pub mod timeout;

pub fn run_test<F, Fut>(name: &str, time_limit: Duration, test_function: F)
where
    F: FnOnce(TestEnvironment) -> Fut,
    Fut: Future<Output = ()>,
{
    let env = TestEnvironment::new(name);

    {
        let env = env.clone();

        // Run the tests and capture a panic.
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let result = test_function(env.clone());
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to build runtime")
                .block_on(async {
                    if timeout(time_limit, result).await.is_err() {
                        panic!("Test timed out after {} seconds.", time_limit.as_secs());
                    }
                });
        }));

        // Run the cleanup. We run this in a separate runtime so that it is run
        // regardless of whether the test panicked or not.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build runtime")
            .block_on(async {
                env.cleanup().await;
            });

        // If the test panicked, resume the panic.
        if let Err(err) = result {
            tracing::error!("Test panicked: {:?}", err);
            std::panic::resume_unwind(err);
        }
    }
}
