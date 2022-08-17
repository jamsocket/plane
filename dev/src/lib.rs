use std::{
    cell::RefCell,
    fs::{create_dir_all, remove_dir_all},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::mpsc::{channel, Receiver, Sender},
};

pub mod container;
pub mod resources;
pub mod timeout;
mod util;

thread_local! {
    pub static TEST_CONTEXT: RefCell<Option<TestContext>> = RefCell::new(None);
}

pub type BoxedFuture = Pin<Box<dyn Future<Output = Result<(), anyhow::Error>>>>;

pub struct TestContext {
    test_name: String,
    teardown_sender: Sender<BoxedFuture>,
    teardown_receiver: Receiver<BoxedFuture>,
}

impl TestContext {
    pub fn scratch_dir(&self) -> PathBuf {
        PathBuf::from("test-scratch")
            .canonicalize()
            .unwrap()
            .join(&self.test_name)
    }
}

pub fn scratch_dir(name: &str) -> PathBuf {
    let scratch_dir = TEST_CONTEXT.with(|d| d.borrow().as_ref().unwrap().scratch_dir());
    let scratch_dir = scratch_dir.join(name);
    create_dir_all(&scratch_dir).unwrap();
    scratch_dir
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

        remove_dir_all(context.scratch_dir()).unwrap();
        create_dir_all(context.scratch_dir()).unwrap();

        context
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
