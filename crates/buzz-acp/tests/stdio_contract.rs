use std::process::Command;

#[test]
fn startup_and_failure_diagnostics_never_enter_acp_stdout() {
    let output = Command::new(env!("CARGO_BIN_EXE_buzz-acp"))
        .env("BUZZ_RELAY_URL", "ws://127.0.0.1:1")
        .env(
            "BUZZ_PRIVATE_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .env("BUZZ_AGENT_COMMAND", "/definitely/missing/buzz-agent")
        .env("RUST_LOG", "buzz_acp=info")
        .output()
        .expect("buzz-acp process should start");

    assert!(
        !output.status.success(),
        "unreachable relay must fail startup"
    );
    assert!(
        output.stdout.is_empty(),
        "ACP stdout contained diagnostics: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("buzz-acp starting:"),
        "missing startup INFO: {stderr}"
    );
    assert!(
        stderr.contains("Error:") || stderr.contains("error"),
        "missing failure diagnostic: {stderr}"
    );
}

/// A `buzz-acp` command with no inherited credentials of either kind.
fn harness_without_credentials() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_buzz-acp"));
    for name in [
        "BUZZ_PRIVATE_KEY",
        "BUZZ_ACP_PRIVATE_KEY",
        "BUZZ_BOT_TOKEN",
        "BUZZ_BOT_TOKEN_EXPIRES_AT",
        "BUZZ_ACCESS_TOKEN",
        "BUZZ_TOKEN_BROKER_URL",
        "BUZZ_TOKEN_BROKER_SECRET",
        "BUZZ_AUTH_TAG",
    ] {
        cmd.env_remove(name);
    }
    cmd.env("BUZZ_AGENT_COMMAND", "/definitely/missing/buzz-agent")
        .env("RUST_LOG", "buzz_acp=info");
    cmd
}

#[test]
fn neither_bot_token_nor_private_key_is_a_configuration_error() {
    let output = harness_without_credentials()
        .env("BUZZ_RELAY_URL", "ws://127.0.0.1:1")
        .output()
        .expect("buzz-acp process should start");
    assert!(!output.status.success());
    assert_ne!(
        output.status.code(),
        Some(78),
        "not an auth-terminal failure"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("configuration error") && stderr.contains("BUZZ_BOT_TOKEN"),
        "{stderr}"
    );
    assert!(output.stdout.is_empty());
}

/// One-request HTTP server answering every request with `status` + `body`.
fn serve_auth(status: &'static str, body: &'static str) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for stream in listener.incoming().take(4) {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    format!("ws://{addr}")
}

/// A bot token the relay already revoked ends the process with exit code 78
/// (auth terminal) so Buzz Desktop reissues the token instead of crash-looping,
/// and the token never appears in the diagnostics.
#[test]
fn revoked_bot_token_exits_auth_terminal_78() {
    let relay = serve_auth(
        "401 Unauthorized",
        r#"{"error":"authentication failed","code":"token_revoked"}"#,
    );
    let output = harness_without_credentials()
        .env("BUZZ_RELAY_URL", relay)
        .env("BUZZ_BOT_TOKEN", "bzb_revokedtokenvalue")
        .output()
        .expect("buzz-acp process should start");
    assert_eq!(output.status.code(), Some(78), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("bzb_revokedtokenvalue"),
        "token leaked: {stderr}"
    );
    assert!(output.stdout.is_empty());
}
