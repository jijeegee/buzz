//! Process-level tests for `git-credential-buzz`: the real binary, real stdin.

use std::io::Write;
use std::process::{Command, Stdio};

fn run(args: &[&str], env: &[(&str, &str)], stdin: &str) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_git-credential-buzz"));
    cmd.args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        // Windows sockets need SystemRoot.
        .env(
            "SYSTEMROOT",
            std::env::var("SYSTEMROOT").unwrap_or_default(),
        )
        // Keep the user's git config out of the test.
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn helper");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn answers_the_relay_host_with_the_bot_token() {
    let (code, out) = run(
        &["get"],
        &[
            ("BUZZ_RELAY_URL", "ws://127.0.0.1:3000"),
            ("BUZZ_BOT_TOKEN", "bzb_abc"),
        ],
        "protocol=http\nhost=127.0.0.1:3000\npath=owner/repo.git/info/refs\n\n",
    );
    assert_eq!(code, 0);
    assert_eq!(out, "username=token\npassword=bzb_abc\n\n");
}

#[test]
fn never_releases_the_token_to_another_host() {
    let (code, out) = run(
        &["get"],
        &[
            ("BUZZ_RELAY_URL", "ws://127.0.0.1:3000"),
            ("BUZZ_BOT_TOKEN", "bzb_abc"),
        ],
        "protocol=https\nhost=github.com\n\n",
    );
    assert_eq!(code, 0);
    assert!(out.is_empty(), "{out}");
}

#[test]
fn store_and_erase_are_silent() {
    for op in ["store", "erase"] {
        let (code, out) = run(
            &[op],
            &[("BUZZ_RELAY_URL", "ws://h"), ("BUZZ_BOT_TOKEN", "bzb_abc")],
            "protocol=http\nhost=h\n\n",
        );
        assert_eq!((code, out.as_str()), (0, ""));
    }
}

#[test]
fn fetches_from_the_broker() {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let n = stream.read(&mut buf).unwrap();
        let req = String::from_utf8_lossy(&buf[..n]).into_owned();
        let body = r#"{"token":"bzb_brokered"}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(resp.as_bytes()).unwrap();
        req
    });
    let (code, out) = run(
        &["get"],
        &[
            ("BUZZ_RELAY_URL", "wss://relay.example"),
            ("BUZZ_TOKEN_BROKER_URL", &url),
            ("BUZZ_TOKEN_BROKER_SECRET", "s3cret"),
        ],
        "protocol=https\nhost=relay.example\n\n",
    );
    assert_eq!(code, 0);
    assert_eq!(out, "username=token\npassword=bzb_brokered\n\n");
    assert!(server
        .join()
        .unwrap()
        .contains("Authorization: Bearer s3cret"));
}

#[test]
fn refuses_plain_http_to_a_tls_relay_host() {
    let (code, out) = run(
        &["get"],
        &[
            ("BUZZ_RELAY_URL", "wss://relay.example"),
            ("BUZZ_BOT_TOKEN", "bzb_abc"),
        ],
        "protocol=http\nhost=relay.example\n\n",
    );
    assert_eq!(code, 0);
    assert!(out.is_empty(), "{out}");

    let (code, out) = run(
        &["get"],
        &[
            ("BUZZ_RELAY_URL", "wss://relay.example"),
            ("BUZZ_BOT_TOKEN", "bzb_abc"),
        ],
        "protocol=https\nhost=relay.example\n\n",
    );
    assert_eq!(code, 0);
    assert_eq!(out, "username=token\npassword=bzb_abc\n\n");
}
