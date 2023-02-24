use crate::{
    messages::logging::{Component, LogMessage, SerializableLevel},
    nats::TypedNats,
};
use anyhow::Result;
use chrono::Utc;
use std::{collections::BTreeMap, fmt::Debug};
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::{
    field::{Field, Visit},
    span::Attributes,
    Id,
};
use tracing_subscriber::{layer::Context, util::SubscriberInitExt, EnvFilter, Layer};
use tracing_subscriber::{layer::SubscriberExt, registry::LookupSpan};

const TRACE_STACKDRIVER: &str = "TRACE_STACKDRIVER";
const LOG_DEFAULT: &str = "info,sqlx=warn";

async fn do_logs(nc: &TypedNats, mut recv: tokio::sync::mpsc::Receiver<LogMessage>) {
    while let Some(msg) = recv.recv().await {
        if let Err(err) = nc.publish(&msg).await {
            println!("{:?}", err);
        }
    }
}

pub struct TracingHandle {
    recv: Option<Receiver<LogMessage>>,
}

impl TracingHandle {
    pub fn init(component: Component) -> Result<Self> {
        let (send, recv) = tokio::sync::mpsc::channel::<LogMessage>(128);

        let filter_layer =
            EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new(LOG_DEFAULT))?;

        let registry = tracing_subscriber::registry()
            .with(LogManagerLogger::new(send, component))
            .with(filter_layer);

        let trace_stackdriver = std::env::var(TRACE_STACKDRIVER).is_ok();
        if trace_stackdriver {
            registry.with(tracing_stackdriver::layer()).init();
        } else {
            registry.with(tracing_subscriber::fmt::layer()).init();
        };

        Ok(TracingHandle { recv: Some(recv) })
    }

    pub fn attach_nats(&mut self, nats: TypedNats) -> Result<()> {
        let recv = anyhow::Context::context(
            self.recv.take(),
            "connect_nats on TracingHandle should not be called more than once.",
        )?;
        tokio::spawn(async move {
            do_logs(&nats, recv).await;
            tracing::error!("do_logs terminated.");
        });

        Ok(())
    }
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
    component: Component,
}

impl LogManagerLogger {
    fn new(sender: Sender<LogMessage>, component: Component) -> LogManagerLogger {
        LogManagerLogger { sender, component }
    }
}

impl<S> Layer<S> for LogManagerLogger
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        // Adapted from:
        // https://github.com/LukeMathWalker/tracing-bunyan-formatter/blob/master/src/storage_layer.rs
        let span = ctx.span(id).expect("Span not found, this is a bug");

        let mut visitor = if let Some(parent_span) = span.parent() {
            let mut extensions = parent_span.extensions_mut();
            extensions
                .get_mut::<JsonVisitor>()
                .map(|v| v.clone())
                .unwrap_or_default()
        } else {
            JsonVisitor::default()
        };

        let mut extensions = span.extensions_mut();

        attrs.record(&mut visitor);
        extensions.insert(visitor);
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let mut visitor = if let Some(span) = ctx.lookup_current() {
            let extensions = span.extensions();
            if let Some(visitor) = extensions.get::<JsonVisitor>() {
                visitor.clone()
            } else {
                JsonVisitor::default()
            }
        } else {
            JsonVisitor::default()
        };

        event.record(&mut visitor);

        let output = LogMessage {
            component: self.component.clone(),
            target: event.metadata().target().to_string(),
            name: event.metadata().name().to_string(),
            severity: SerializableLevel(*event.metadata().level()),
            time: Utc::now(),
            fields: visitor.0,
        };

        // make sure logging in this function call does not trigger an infinite loop
        // self.nc.publish_drone_log_message(&msg).unwrap();
        if self.sender.try_send(output).is_err() {
            println!("Warning: sender buffer is full.");
        }
    }
}

#[derive(Clone, Default)]
struct JsonVisitor(BTreeMap<String, serde_json::Value>);

impl Visit for JsonVisitor {
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
        // If a field value is a serde_json Value type, it will
        // stringify to valid JSON. Since we are outputting JSON,
        // rather than encoding JSON as a string, we replace it
        // with the JSON-decoded string.
        // This is kind of inefficient, because we pass a Value
        // through an encode/decode cycle just to get back the
        // same value, but it's the best we can do (see
        // https://github.com/tokio-rs/tracing/issues/663)
        let value = format!("{:?}", value);
        match serde_json::from_str::<serde_json::Value>(&value) {
            Ok(value) => {
                self.0.insert(field.name().into(), value);
            }
            Err(_) => {
                self.0.insert(field.name().into(), value.into());
            }
        }
    }
}
