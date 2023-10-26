use std::fmt::Debug;

pub trait LogAndIgnoreError {
    fn log_and_ignore_error(self);
}

impl<T, E: Debug> LogAndIgnoreError for Result<T, E> {
    fn log_and_ignore_error(self) {
        if let Err(error) = self {
            tracing::error!(?error);
        }
    }
}
