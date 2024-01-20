use opentelemetry::{
    trace::{TraceContextExt, Tracer},
    KeyValue,
};
use opentelemetry_sdk::{
    runtime,
    trace::{self, BatchConfig},
    Resource,
};
use opentelemetry_semantic_conventions::{
    resource::{DEPLOYMENT_ENVIRONMENT, SERVICE_NAME, SERVICE_VERSION},
    SCHEMA_URL,
};
use tracing::{error, span, info};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{
    fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter,
};

fn resource() -> Resource {
    Resource::from_schema_url(
        [
            KeyValue::new(SERVICE_NAME, env!("CARGO_PKG_NAME")),
            KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
            KeyValue::new(DEPLOYMENT_ENVIRONMENT, "develop"),
        ],
        SCHEMA_URL,
    )
}

pub fn init_tracing() {
    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_trace_config(trace::config().with_resource(resource()))
        .with_batch_config(BatchConfig::default())
        .with_exporter(opentelemetry_otlp::new_exporter().tonic())
        .install_batch(runtime::Tokio)
        .unwrap();

    tracer.in_span("doing_work", |cx| {
        // Traced app logic here...
        cx.span().add_event("Did some work!".to_string(), vec![]);
    });

    let filter_str: &str = "warn,plane=info";

    let otel_layer = OpenTelemetryLayer::new(tracer);

    let fmt_layer = fmt::layer().with_target(false);

    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new(filter_str))
        .with(otel_layer)
        .with(fmt_layer);

    subscriber.init();

    let root = span!(tracing::Level::INFO, "app_starta", work_units = 2);
    let _guard = root.enter();
    error!("here1");
    info!("here2");
}
