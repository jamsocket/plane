use crate::{
    messages::logging::{LogMessage, SerializableLevel},
    nats::TypedNats,
};
use anyhow::Result;
use chrono::Utc;
use std::{collections::BTreeMap, fmt::Debug};
use tokio::sync::mpsc::Sender;
use tracing::field::{Field, Visit};
use tracing_stackdriver::Stackdriver;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{layer::Context, util::SubscriberInitExt, EnvFilter, Layer};

const TRACE_STACKDRIVER: &str = "TRACE_STACKDRIVER";

async fn do_logs(nc: &TypedNats, mut recv: tokio::sync::mpsc::Receiver<LogMessage>) {
    while let Some(msg) = recv.recv().await {
        if let Err(err) = nc.publish(&LogMessage::subject(), &msg).await {
            println!("{:?}", err);
        }
    }
}

pub fn init_tracing() -> Result<()> {
    let filter_layer =
        EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new("info,sqlx=warn"))?;

    let registry = tracing_subscriber::registry().with(filter_layer);

    let trace_stackdriver = std::env::var(TRACE_STACKDRIVER).is_ok();
    if trace_stackdriver {
        registry.with(Stackdriver::default()).init();
    } else {
        registry.with(tracing_subscriber::fmt::layer()).init();
    };

    Ok(())
}

pub fn init_tracing_with_nats(nats: TypedNats) -> Result<()> {
    let (send, recv) = tokio::sync::mpsc::channel::<LogMessage>(128);

    let filter_layer =
        EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new("info,sqlx=warn"))?;

    let registry = tracing_subscriber::registry()
        .with(LogManagerLogger::new(send))
        .with(filter_layer);

    let trace_stackdriver = std::env::var(TRACE_STACKDRIVER).is_ok();
    if trace_stackdriver {
        registry.with(Stackdriver::default()).init();
    } else {
        registry.with(tracing_subscriber::fmt::layer()).init();
    };

    tokio::spawn(async move {
        let result = do_logs(&nats, recv).await;
        tracing::error!(?result, "do_logs terminated.");
    });

    Ok(())
}

/// Helper trait for consuming and logging errors without further processing.
/// Mostly useful for when a function returns a `Result<()>` and we want to
/// know if it failed but still continue execution.
pub trait LogError<E: Debug> {
    fn log_error(&self, message: &str);
}

impl<T, E: Debug> LogError<E> for Result<T, E> {
    fn log_error(&self, message: &str) {
        if let Err(error) = self {
            tracing::error!(?error, message);
        }
    }
}

impl<T> LogError<()> for Option<T> {
    fn log_error(&self, message: &str) {
        if self.is_none() {
            tracing::error!(message);
        }
    }
}

pub struct LogManagerLogger {
    sender: Sender<LogMessage>,
}

impl LogManagerLogger {
    fn new(sender: Sender<LogMessage>) -> LogManagerLogger {
        LogManagerLogger { sender }
    }
}

impl<S> Layer<S> for LogManagerLogger
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = BTreeMap::new();
        let mut visitor = JsonVisitor(&mut fields);
        event.record(&mut visitor);

        let output = LogMessage {
            target: event.metadata().target().to_string(),
            name: event.metadata().name().to_string(),
            severity: SerializableLevel(*event.metadata().level()),
            time: Utc::now(),
            fields,
        };

        // make sure logging in this function call does not trigger an infinite loop
        // self.nc.publish_drone_log_message(&msg).unwrap();
        if self.sender.try_send(output).is_err() {
            println!("Warning: sender buffer is full.");
        }
    }
}

struct JsonVisitor<'a>(&'a mut BTreeMap<String, serde_json::Value>);

impl<'a> Visit for JsonVisitor<'a> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.0
            .insert(field.name().to_string(), serde_json::json!(value));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0
            .insert(field.name().to_string(), serde_json::json!(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0
            .insert(field.name().to_string(), serde_json::json!(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0
            .insert(field.name().to_string(), serde_json::json!(value));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0
            .insert(field.name().to_string(), serde_json::json!(value));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.0.insert(
            field.name().to_string(),
            serde_json::Value::from(format!("{:?}", value)),
        );
    }
}
