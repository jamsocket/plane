use futures::Stream;
use plane_core::types::BackendId;
use std::pin::Pin;

pub trait Engine {
    /// Returns an async stream which yields a backend ID when the status of that
    /// backend has changed. This causes the Plane-side control loop to request
    /// state information from the engine.
    fn interrupt_stream(&self) -> Pin<Box<dyn Stream<Item=BackendId> + Send>>;
}