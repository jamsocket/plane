use std::collections::BTreeMap;
use hyper::{Client, Request, header::{HeaderName, HeaderValue}};
use hyper_tls::HttpsConnector;
use serde_json::json;
use tracing::{field::Visit, Event, Subscriber, Level};
use tracing_subscriber::{layer::Context, Layer};

fn send_message(endpoint: &str, message: &str) {
    let https = HttpsConnector::new();
    let client = Client::builder().build::<_, hyper::Body>(https);

    let body = json!({
        "text": message,
    });

    let result = client.request(
        Request::builder()
            .uri(endpoint)
            .method("POST")
            .header(
                HeaderName::from_static("content-type"),
                HeaderValue::from_static("application/json"),
            )
            .body(
                serde_json::to_vec(&body)
                    .expect("Message serialization shouldn't fail.")
                    .into(),
            )
            .expect("Request construction shouldn't fail."),
    );

    tokio::spawn(async {
        // We don't log an error, because that would cause an infinite loop (until we have better filtering).
        result.await
    });
}

#[derive(Default)]
struct SlackMessageVisitor {
    pub fields: BTreeMap<String, String>,
}

impl Visit for SlackMessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{:?}", value));
    }
}

pub struct SlackLayer {
    endpoint_url: String,
}

impl SlackLayer {
    pub fn new(endpoint_url: &str) -> Self {
        SlackLayer {
            endpoint_url: endpoint_url.to_string(),
        }
    }
}

impl<S: Subscriber> Layer<S> for SlackLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();

        let level = metadata.level();
        let target = metadata.target();

        if ignore_log(*level) {
            // TODO: use tracing filter for this?
            return;
        }

        let mut visitor = SlackMessageVisitor::default();
        event.record(&mut visitor);

        let fields_message_parts: Vec<String> = visitor
            .fields
            .into_iter()
            .map(|(key, value)| format!("*{}*: {}", key, value))
            .collect();
        let fields_message = fields_message_parts.join("\n");

        let message = format!("*{}* {}\n{}", level, target, fields_message);

        send_message(&self.endpoint_url, &message);
    }
}

fn ignore_log(level: Level) -> bool {
    level > Level::INFO
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_ignore_log() {
        assert!(ignore_log(Level::DEBUG));
        assert!(ignore_log(Level::TRACE));
        assert!(!ignore_log(Level::ERROR));
        assert!(!ignore_log(Level::INFO));
        assert!(!ignore_log(Level::WARN));
    }
}
