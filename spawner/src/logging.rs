use std::fmt::Debug;

use axum::http::StatusCode;
use tracing_subscriber::EnvFilter;

const LOG_MODULES: &[&str] = &["spawner"];

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

pub type WebResult<T> = std::result::Result<T, StatusCode>;

pub trait LogError<T> {
    fn log_error_internal(self) -> WebResult<T>;
    fn log_error_bad_request(self) -> WebResult<T>;
    fn log_error_not_found(self) -> WebResult<T>;
    fn log_error_forbidden(self) -> WebResult<T>;
}

impl<T, E> LogError<T> for Result<T, E>
where
    E: Debug,
{
    fn log_error_not_found(self) -> WebResult<T> {
        match self {
            Ok(v) => Ok(v),
            Err(error) => {
                tracing::error!(?error, "Error: {:?}", error);

                Err(StatusCode::NOT_FOUND)
            }
        }
    }

    fn log_error_forbidden(self) -> WebResult<T> {
        match self {
            Ok(v) => Ok(v),
            Err(error) => {
                tracing::error!(?error, "Error: {:?}", error);

                Err(StatusCode::FORBIDDEN)
            }
        }
    }

    fn log_error_internal(self) -> WebResult<T> {
        match self {
            Ok(v) => Ok(v),
            Err(error) => {
                tracing::error!(?error, "Error: {:?}", error);

                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }

    fn log_error_bad_request(self) -> WebResult<T> {
        match self {
            Ok(v) => Ok(v),
            Err(error) => {
                tracing::error!(?error, "Error: {:?}", error);

                Err(StatusCode::BAD_REQUEST)
            }
        }
    }
}
