use self::{test_env::TestEnvironment, timeout::WithTimeout};
use futures_util::Future;
use plane::{client::PlaneClient, names::BackendName, types::BackendStatus};
use std::{panic::AssertUnwindSafe, time::Duration};
use tokio::time::timeout;

pub mod async_drop;
pub mod auth_mock;
pub mod docker;
pub mod localhost_resolver;
pub mod proxy_mock;
pub mod resources;
pub mod simple_axum_server; // NOTE: copied from dynamic-proxy
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

// For some reason Rust doesn't see that this function is used
#[allow(dead_code)]
pub async fn wait_until_backend_terminated(client: &PlaneClient, backend_id: &BackendName) {
    let mut backend_status_stream = client
        .backend_status_stream(backend_id)
        .with_timeout(10)
        .await
        .unwrap()
        .unwrap();

    while let Ok(Some(message)) = backend_status_stream.next().with_timeout(10).await {
        tracing::info!("Got status: {:?}", message);
        if message.status == BackendStatus::Terminated {
            break;
        }
    }
}
