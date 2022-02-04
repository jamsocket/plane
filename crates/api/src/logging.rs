use tracing_subscriber::EnvFilter;

const LOG_MODULES: &[&str] = &["spawner", "spawner_api", "tower_http"];

pub fn init_logging() {
    let mut env_filter = EnvFilter::default();

    for module in LOG_MODULES {
        env_filter = env_filter.add_directive(
            format!("{}=info", module)
                .parse()
                .expect("Could not parse logging directive"),
        );
    }

    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}
