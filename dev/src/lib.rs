use anyhow::Result;
use std::{
    cell::RefCell,
    env::current_dir,
    fs::{create_dir_all, remove_dir_all},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::mpsc::{channel, Receiver, Sender},
};

pub mod container;
pub mod resources;
pub mod timeout;
pub mod util;

thread_local! {
    pub static TEST_CONTEXT: RefCell<Option<TestContext>> = RefCell::new(None);
}

pub type BoxedFuture = Pin<Box<dyn Future<Output = Result<(), anyhow::Error>>>>;

pub struct TestContext {
    test_name: String,
    teardown_sender: Sender<BoxedFuture>,
    teardown_receiver: Receiver<BoxedFuture>,
}

pub fn scratch_dir(name: &str) -> PathBuf {
    let scratch_dir = TEST_CONTEXT.with(|d| d.borrow().as_ref().unwrap().scratch_dir());
    let scratch_dir = scratch_dir.join(name);
    create_dir_all(&scratch_dir).unwrap();
    scratch_dir
}

pub fn test_name() -> String {
    TEST_CONTEXT.with(|d| d.borrow().as_ref().unwrap().test_name.clone())
}

impl TestContext {
    pub fn new(test_name: &str) -> Self {
        tracing::info!(%test_name, "Created test context.");
        let (teardown_sender, teardown_receiver) = channel();

        let context = TestContext {
            teardown_sender,
            teardown_receiver,
            test_name: test_name.into(),
        };

        let _ = remove_dir_all(context.scratch_dir());
        create_dir_all(context.scratch_dir()).unwrap();

        context
    }

    pub fn scratch_dir(&self) -> PathBuf {
        current_dir()
            .unwrap()
            .join("test-scratch")
            .join(&self.test_name)
    }

    pub fn add_teardown_task<T>(&self, task: T)
    where
        T: Future<Output = Result<(), anyhow::Error>> + 'static,
    {
        self.teardown_sender.send(Box::pin(task)).unwrap();
    }

    pub async fn teardown(&self) {
        while let Ok(task) = self.teardown_receiver.try_recv() {
            if let Err(e) = task.await {
                tracing::error!(?e, "Error encountered during teardown.");
            }
        }
    }
}

pub fn run_test<F>(name: &str, future: F)
where
    F: Future<Output = ()>,
{
    let context = TestContext::new(name);
    TEST_CONTEXT.with(|cell| cell.replace(Some(context)));
    let scratch_dir = scratch_dir("logs");

    let file_appender = tracing_appender::rolling::RollingFileAppender::new(
        tracing_appender::rolling::Rotation::NEVER,
        scratch_dir,
        "test-log.txt",
    );

    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    let subscriber = tracing_subscriber::fmt()
        .compact()
        .with_ansi(false)
        .with_writer(non_blocking)
        .finish();

    let dispatcher = tracing::dispatcher::Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatcher);

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build runtime")
        .block_on(future);

    TEST_CONTEXT.with(|cell| {
        tokio::runtime::Runtime::new()
            .expect("Failed to create runtime")
            .block_on(async {
                let context = cell.borrow_mut().take().expect("Test context not set.");
                context.teardown().await;
            })
    });
}
