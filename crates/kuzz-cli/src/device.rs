//! This process's device signer (device-bound keys plan §6): it adds the
//! `device` tag to NIP-42, NIP-98 and Blossom auth events and signs the
//! `X-Kuzz-Device-Proof` header of account calls (`/auth/*`).
//!
//! The key is this OS user's shared device key, opened, never created,
//! except by `kuzz auth login`. A managed-agent child that cannot open it
//! uses the `kuzz-acp` device broker (event tags only). No key, or a failed
//! signature, is a warning on stderr: the request goes out exactly as it did
//! before, and the relay decides (it refuses only under `enforce`).
//!
//! TODO(device-keys): `kuzz pair` (external pairing, A4) and a long-lived
//! `kuzz messages watch` connection do not exist yet; when they land, sign
//! their auth with [`process_signer`] the same way.

use std::sync::atomic::{AtomicI64, Ordering};

use kuzz_token_broker::device::{tag_or_unchanged, DeviceSigner};
use kuzz_ws_client::DeviceTagger;
use nostr::UnsignedEvent;
use zeroize::Zeroizing;

/// Header carrying a device proof on `/auth/*` calls.
pub const PROOF_HEADER: &str = "X-Kuzz-Device-Proof";
/// Response header with the relay clock (unix seconds).
const SERVER_TIME_HEADER: &str = "x-kuzz-server-time";
/// Proof purpose of a Bearer-authenticated `/auth/*` call.
pub const PURPOSE_AUTH: &str = "auth";
/// Proof purpose of `POST /auth/refresh`.
pub const PURPOSE_REFRESH: &str = "refresh";
/// Proof purpose of a login (`complete`).
pub const PURPOSE_BIND: &str = "bind";
/// Clock differences below this are ignored.
const SKEW_TOLERANCE_SECS: i64 = 5;

#[cfg_attr(test, allow(dead_code))]
static PROCESS: tokio::sync::OnceCell<Option<DeviceSigner>> = tokio::sync::OnceCell::const_new();
/// Relay clock minus ours, from the last `/auth/*` answer.
static CLOCK_OFFSET: AtomicI64 = AtomicI64::new(0);

#[cfg(test)]
thread_local! {
    static TEST_SIGNER: std::cell::RefCell<Option<DeviceSigner>> =
        const { std::cell::RefCell::new(None) };
}

/// Tests: the signer [`process_signer`] returns on this thread.
#[cfg(test)]
pub(crate) fn set_test_signer(signer: Option<DeviceSigner>) {
    TEST_SIGNER.with(|slot| *slot.borrow_mut() = signer);
}

/// Report a device-key problem; the caller carries on without it.
pub fn warn(reason: &str) {
    eprintln!("warning: {reason}; sending without the device signature");
}

/// This process's signer, resolved once (opening the TPM key blocks, so it
/// runs on a blocking thread). Tests see only [`set_test_signer`]'s value.
pub async fn process_signer() -> Option<DeviceSigner> {
    #[cfg(test)]
    {
        TEST_SIGNER.with(|slot| slot.borrow().clone())
    }
    #[cfg(not(test))]
    {
        PROCESS
            .get_or_init(|| async {
                match tokio::task::spawn_blocking(DeviceSigner::from_env).await {
                    Ok((signer, warning)) => {
                        if let Some(warning) = warning {
                            warn(&warning);
                        }
                        signer
                    }
                    Err(error) => {
                        warn(&format!("device key task failed: {error}"));
                        None
                    }
                }
            })
            .await
            .clone()
    }
}

/// `unsigned` with `signer`'s device tag (on a blocking thread), or
/// unchanged with a warning when it cannot be tagged.
pub async fn tag_event(signer: Option<&DeviceSigner>, unsigned: UnsignedEvent) -> UnsignedEvent {
    let Some(signer) = signer.cloned() else {
        return unsigned;
    };
    let fallback = unsigned.clone();
    match tokio::task::spawn_blocking(move || tag_or_unchanged(Some(&signer), unsigned)).await {
        Ok((tagged, None)) => tagged,
        Ok((unchanged, Some(error))) => {
            warn(&error.to_string());
            unchanged
        }
        Err(error) => {
            warn(&format!("device tag task failed: {error}"));
            fallback
        }
    }
}

/// [`tag_event`] with [`process_signer`]: for in-process helpers such as
/// `kuzz-dev-mcp`.
pub async fn tag_with_process_signer(unsigned: UnsignedEvent) -> UnsignedEvent {
    tag_event(process_signer().await.as_ref(), unsigned).await
}

/// A WebSocket AUTH tagger for [`kuzz_ws_client`], or `None` without a signer.
pub fn ws_tagger(signer: Option<DeviceSigner>) -> Option<DeviceTagger> {
    let signer = signer?;
    Some(std::sync::Arc::new(move |unsigned| {
        let (event, error) = tag_or_unchanged(Some(&signer), unsigned);
        if let Some(error) = error {
            warn(&error.to_string());
        }
        event
    }))
}

/// Learn the relay clock from an `/auth/*` response, so later proofs are
/// stamped in relay time (the relay accepts ±60 s).
pub fn note_server_time(headers: &reqwest::header::HeaderMap) {
    let Some(server) = headers
        .get(SERVER_TIME_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok())
    else {
        return;
    };
    let offset = server - unix_now();
    let offset = if offset.abs() < SKEW_TOLERANCE_SECS {
        0
    } else {
        offset
    };
    CLOCK_OFFSET.store(offset, Ordering::Relaxed);
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

/// The `X-Kuzz-Device-Proof` value for an account call: `purpose` over
/// `binding` (`<METHOD> <full URL>` or `login:<sha256 hex>`), the token the
/// call carries (`None` for a login) and its body. `None` (with a warning on
/// failure) when this process holds no local key: the broker never signs
/// header proofs.
pub async fn header_proof(
    signer: Option<&DeviceSigner>,
    purpose: &'static str,
    binding: String,
    token: Option<&str>,
    body: &[u8],
) -> Option<String> {
    let key = signer?.key()?.clone();
    let token = token.map(|token| Zeroizing::new(token.to_owned()));
    let body = body.to_vec();
    let ts = unix_now() + CLOCK_OFFSET.load(Ordering::Relaxed);
    let signed = tokio::task::spawn_blocking(move || {
        key.header_proof_at(
            ts,
            purpose,
            &binding,
            token.as_ref().map(|token| token.as_str()),
            &body,
        )
    })
    .await;
    match signed {
        Ok(Ok(proof)) => Some(proof),
        Ok(Err(error)) => {
            warn(&format!("device proof failed: {error}"));
            None
        }
        Err(error) => {
            warn(&format!("device proof task failed: {error}"));
            None
        }
    }
}

/// [`header_proof`] for a request: purpose over `<method> <url>`.
pub async fn request_proof(
    signer: Option<&DeviceSigner>,
    purpose: &'static str,
    method: &str,
    url: &str,
    token: &str,
    body: &[u8],
) -> Option<String> {
    header_proof(
        signer,
        purpose,
        format!("{method} {url}"),
        Some(token),
        body,
    )
    .await
}

/// Add [`PROOF_HEADER`] to `request` when a proof could be signed.
pub fn with_proof(
    request: reqwest::RequestBuilder,
    proof: Option<String>,
) -> reqwest::RequestBuilder {
    match proof {
        Some(proof) => request.header(PROOF_HEADER, proof),
        None => request,
    }
}

/// Whether an `/auth/*` 401 refused the device proof's time (worth one
/// retry after [`note_server_time`]).
pub fn is_stale_proof(status: u16, body: &str) -> bool {
    status == 401
        && serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_owned))
            .as_deref()
            == Some("device_proof_stale")
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use kuzz_auth::device_proof::{DeviceProof, ProofScope};
    use kuzz_token_broker::device::DeviceKey;

    /// A software device key in a temp dir (the dir must outlive it).
    pub fn file_signer(dir: &tempfile::TempDir) -> (DeviceSigner, DeviceKey) {
        let key = DeviceKey::open_or_create_file(&dir.path().join("device.key")).expect("key");
        (DeviceSigner::Key(key.clone()), key)
    }

    /// Verify `header` as the relay does for `purpose`, `binding`, `token` and `body`.
    pub fn verify_proof(
        key: &DeviceKey,
        header: &str,
        purpose: &str,
        binding: &str,
        token: Option<&str>,
        body: &[u8],
    ) -> bool {
        use sha2::Digest as _;
        let proof = DeviceProof::parse(header).expect("proof parses");
        let token_hash = token.map(kuzz_auth::hash_token);
        let body_hash: Option<[u8; 32]> =
            (!body.is_empty()).then(|| sha2::Sha256::digest(body).into());
        let scope = ProofScope {
            purpose,
            binding,
            token_hash: token_hash.as_ref(),
            body_hash: body_hash.as_ref(),
        };
        proof.verify(&key.pubkey(), &scope, unix_now()).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[tokio::test]
    async fn request_proofs_verify_for_their_binding_token_and_body() {
        let dir = tempfile::tempdir().unwrap();
        let (signer, key) = file_signer(&dir);
        let url = "https://relay.example/auth/me";
        let proof = request_proof(Some(&signer), PURPOSE_AUTH, "GET", url, "bzs_tok", b"")
            .await
            .expect("proof");
        assert!(verify_proof(
            &key,
            &proof,
            "auth",
            &format!("GET {url}"),
            Some("bzs_tok"),
            b""
        ));
        assert!(!verify_proof(
            &key,
            &proof,
            "auth",
            &format!("POST {url}"),
            Some("bzs_tok"),
            b""
        ));
        assert!(!verify_proof(
            &key,
            &proof,
            "refresh",
            &format!("GET {url}"),
            Some("bzs_tok"),
            b""
        ));
    }

    #[tokio::test]
    async fn no_local_key_means_no_proof() {
        assert!(
            request_proof(None, PURPOSE_AUTH, "GET", "https://r/auth/me", "t", b"")
                .await
                .is_none()
        );
        let broker = DeviceSigner::Broker {
            url: "http://127.0.0.1:1".into(),
            secret: kuzz_token_broker::Secret::new("s".into()),
        };
        assert!(
            request_proof(
                Some(&broker),
                PURPOSE_AUTH,
                "GET",
                "https://r/auth/me",
                "t",
                b""
            )
            .await
            .is_none(),
            "the broker never signs header proofs"
        );
    }

    #[test]
    fn stale_proof_answers_are_recognised() {
        assert!(is_stale_proof(401, r#"{"code":"device_proof_stale"}"#));
        assert!(!is_stale_proof(401, r#"{"code":"token_expired"}"#));
        assert!(!is_stale_proof(400, r#"{"code":"device_proof_stale"}"#));
    }
}
