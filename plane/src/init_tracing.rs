use tracing_subscriber::{
    filter::LevelFilter, fmt::format::FmtSpan, prelude::__tracing_subscriber_SubscriberExt,
    util::SubscriberInitExt, EnvFilter,
};

pub fn init_tracing() {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    // Use JSON if PLANE_LOG_JSON is set to anything other than "false".
    let use_json = std::env::var("PLANE_LOG_JSON")
        .map(|s| s != "false")
        .unwrap_or_default();

    if use_json {
        let layer = tracing_subscriber::fmt::layer().json();
        tracing_subscriber::registry()
            .with(layer)
            .with(filter)
            .init();
        return;
    } else {
        let layer = tracing_subscriber::fmt::layer().with_span_events(FmtSpan::FULL);

        tracing_subscriber::registry()
            .with(layer)
            .with(filter)
            .init();
    }
}
