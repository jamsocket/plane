//! Near-identical copy of hyper_util::server::graceful::GracefulShutdown
//! that derives `Clone` and adds a `subscribe` method.
//! https://github.com/hyperium/hyper-util/blob/master/src/server/graceful.rs

use hyper_util::server::graceful::GracefulConnection;
use pin_project_lite::pin_project;
use std::{
    fmt::{self, Debug},
    future::Future,
    pin::Pin,
    task::{self, Poll},
};
use tokio::sync::watch;

#[derive(Clone)]
pub struct GracefulShutdown {
    tx: watch::Sender<()>,
}

impl GracefulShutdown {
    /// Create a new graceful shutdown helper.
    pub fn new() -> Self {
        let (tx, _) = watch::channel(());
        Self { tx }
    }

    /// Wrap a future for graceful shutdown watching.
    pub fn watch<C: GracefulConnection>(&self, conn: C) -> impl Future<Output = C::Output> {
        let mut rx = self.tx.subscribe();
        GracefulConnectionFuture::new(conn, async move {
            let _ = rx.changed().await;
            // hold onto the rx until the watched future is completed
            rx
        })
    }

    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.tx.subscribe()
    }

    /// Signal shutdown for all watched connections.
    ///
    /// This returns a `Future` which will complete once all watched
    /// connections have shutdown.
    pub async fn shutdown(self) {
        let Self { tx } = self;

        // signal all the watched futures about the change
        let _ = tx.send(());
        // and then wait for all of them to complete
        tx.closed().await;
    }
}

pin_project! {
    struct GracefulConnectionFuture<C, F: Future> {
        #[pin]
        conn: C,
        #[pin]
        cancel: F,
        #[pin]
        // If cancelled, this is held until the inner conn is done.
        cancelled_guard: Option<F::Output>,
    }
}

impl<C, F: Future> GracefulConnectionFuture<C, F> {
    fn new(conn: C, cancel: F) -> Self {
        Self {
            conn,
            cancel,
            cancelled_guard: None,
        }
    }
}

impl<C, F: Future> Debug for GracefulConnectionFuture<C, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GracefulConnectionFuture").finish()
    }
}

impl<C, F> Future for GracefulConnectionFuture<C, F>
where
    C: GracefulConnection,
    F: Future,
{
    type Output = C::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        if this.cancelled_guard.is_none() {
            if let Poll::Ready(guard) = this.cancel.poll(cx) {
                this.cancelled_guard.set(Some(guard));
                this.conn.as_mut().graceful_shutdown();
            }
        }
        this.conn.poll(cx)
    }
}
