const DEFAULT_DATABASE_URL: &str = "postgres://kuzz:kuzz_dev@localhost:5432/kuzz"; // sadscan:disable np.postgres.1 -- local test-only credentials

/// Resolve the database URL shared by PostgreSQL-backed unit tests.
pub(crate) fn database_url() -> String {
    std::env::var("KUZZ_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("TEST_DATABASE_URL"))
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned())
}
