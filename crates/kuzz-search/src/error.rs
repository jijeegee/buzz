use thiserror::Error;

/// Errors produced by the FTS service.
#[derive(Debug, Error)]
pub enum SearchError {
    /// A database error from sqlx.
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

impl SearchError {
    /// True when Postgres cancelled the search for running past its
    /// execution budget (`statement_timeout`, SQLSTATE 57014).
    pub fn is_statement_cancelled(&self) -> bool {
        matches!(
            self,
            SearchError::Db(sqlx::Error::Database(db)) if db.code().as_deref() == Some("57014")
        )
    }
}
