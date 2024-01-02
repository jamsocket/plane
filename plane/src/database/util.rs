/// Unique violation error code in Postgres.
/// From: https://www.postgresql.org/docs/9.2/errcodes-appendix.html
pub const PG_UNIQUE_VIOLATION_ERROR: &str = "23505";
pub const PG_FOREIGN_KEY_VIOLATION_ERROR: &str = "23503";

pub fn unique_violation_to_option<T>(result: sqlx::Result<T>) -> sqlx::Result<Option<T>> {
    match result {
        Ok(result) => Ok(Some(result)),
        Err(sqlx::Error::Database(db_error)) => {
            if db_error
                .code()
                .map(|d| d == PG_UNIQUE_VIOLATION_ERROR)
                .unwrap_or_default()
            {
                Ok(None)
            } else {
                Err(sqlx::Error::Database(db_error))
            }
        }
        Err(other) => Err(other),
    }
}

pub trait MapSqlxError<T> {
    fn map_sqlx_error(self) -> Result<T, sqlx::Error>;
}

impl<T> MapSqlxError<T> for serde_json::Result<T> {
    fn map_sqlx_error(self) -> Result<T, sqlx::Error> {
        self.map_err(|e| sqlx::Error::Decode(Box::new(e)))
    }
}
