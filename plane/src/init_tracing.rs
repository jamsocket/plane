use tracing_subscriber::{
    filter::LevelFilter, fmt::format::FmtSpan, prelude::__tracing_subscriber_SubscriberExt,
    util::SubscriberInitExt, EnvFilter,
};

pub fn init_tracing() {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    let layer = tracing_subscriber::fmt::layer().with_span_events(FmtSpan::FULL);

    tracing_subscriber::registry()
        .with(layer)
        .with(filter)
        .init();
}
