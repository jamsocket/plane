pub mod cli;
pub mod logging;
pub mod messages;
pub mod nats;
pub mod nats_connection;
pub mod retry;
pub mod state;
pub mod supervisor;
pub mod timing;
pub mod types;

/// This is a stand-in for the “never” type until RFC 1216 is stabilized.
/// Because it is not constructable, the compiler enforces that a function
/// which returns it may never terminate.
#[derive(Debug)]
pub enum Never {}

/// Represents a `Result` whose `Ok()` variant can never be constructed.
/// A function which returns this may only terminate in an error state.
/// It is used as a return value to represent functions that are expected
/// to run forever, but may error-out.
pub type NeverResult = anyhow::Result<Never>;
