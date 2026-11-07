//! `kuzz-admin settings` through the real binary against PostgreSQL.

use serde_json::Value;
use sqlx::PgPool;
use tokio::process::Command;

async fn admin(url: &str, args: &[&str]) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_kuzz-admin"))
        .args(args)
        .env("DATABASE_URL", url)
        .output()
        .await
        .expect("run kuzz-admin");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = stdout
        .lines()
        .last()
        .and_then(|line| serde_json::from_str(line).ok())
        .unwrap_or(Value::Null);
    (output.status.success(), json)
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn settings_set_and_get_window_check_mode() {
    let url = std::env::var("KUZZ_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("TEST_DATABASE_URL"))
        .expect("KUZZ_TEST_DATABASE_URL");
    // Same advisory lock as kuzz-relay's window check mode tests: the row is
    // deployment-global, so writers in other test binaries must wait.
    let mut lock = <sqlx::PgConnection as sqlx::Connection>::connect(&url)
        .await
        .expect("lock session");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(0x6b75_7a7a_7763_i64)
        .execute(&mut lock)
        .await
        .expect("window check mode test lock");
    let pool = PgPool::connect(&url).await.expect("connect");
    let clear = || async {
        sqlx::query("DELETE FROM relay_runtime_settings WHERE key = 'window_check_mode'")
            .execute(&pool)
            .await
            .expect("clear setting");
    };
    clear().await;

    let (ok, unset) = admin(&url, &["settings", "get", "window_check_mode"]).await;
    assert!(ok);
    assert_eq!(unset["value"], "off", "unset mode defaults to off");
    assert_eq!(unset["stored"], Value::Null);

    let (ok, set) = admin(&url, &["settings", "set", "window_check_mode", "shadow"]).await;
    assert!(ok, "{set}");
    assert_eq!(set["value"], "shadow");
    let (ok, got) = admin(&url, &["settings", "get", "window_check_mode"]).await;
    assert!(ok);
    assert_eq!(got["value"], "shadow");

    let (ok, _) = admin(&url, &["settings", "set", "window_check_mode", "maybe"]).await;
    assert!(!ok, "invalid values are refused");
    let (ok, _) = admin(&url, &["settings", "set", "unknown_key", "on"]).await;
    assert!(!ok, "unknown keys are refused");
    let (_, got) = admin(&url, &["settings", "get", "window_check_mode"]).await;
    assert_eq!(got["value"], "shadow", "refused sets change nothing");

    clear().await;
}
