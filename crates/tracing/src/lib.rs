use crate::slack::SlackLayer;
use tracing::Subscriber;
use tracing_stackdriver::Stackdriver;
use tracing_subscriber::{layer::SubscriberExt, registry::LookupSpan, EnvFilter, Registry};

mod slack;

trait NewTrait: Subscriber + Send + Sync + 'static + for<'span> LookupSpan<'span> {}

pub fn init_logging() {
    let env_filter = EnvFilter::from_default_env();
    let trace_stackdriver = std::env::var("TRACE_STACKDRIVER").is_ok();

    if trace_stackdriver {
        let stackdriver = Stackdriver::default();

        if let Some(slack_endpoint) = std::env::var("TRACE_SLACK_URL").ok() {
            let slack_layer = SlackLayer::new(&slack_endpoint);

            let subscriber = Registry::default()
                .with(slack_layer)
                .with(stackdriver)
                .with(env_filter);

            tracing::subscriber::set_global_default(subscriber)
                .expect("Could not set up global logger");
        } else {
            let subscriber = Registry::default()
                .with(stackdriver)
                .with(env_filter);

            tracing::subscriber::set_global_default(subscriber)
                .expect("Could not set up global logger");
        }
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .init();
    }
}
