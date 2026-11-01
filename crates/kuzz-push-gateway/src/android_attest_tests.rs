//! Android Key Attestation against synthetic chains generated per test run
//! with rcgen. These roots are test-only and never trusted in production.
use super::*;
use rcgen::{
    BasicConstraints, CertificateParams, CustomExtension, DnType, IsCa, Issuer, KeyPair,
    PublicKeyData, SerialNumber, SigningKey, PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384,
};
use std::{collections::VecDeque, sync::Mutex};

pub(crate) const PACKAGE: &str = "app.kuzz.mobile";
const T0: i64 = 1_800_000_000;
const ROOT_SERIAL: u8 = 0x51;
const INTERMEDIATE_SERIAL: [u8; 2] = [0x0a, 0x0b];
const LEAF_SERIAL: [u8; 2] = [0x0c, 0x0d];
pub(crate) const HARDWARE_ROOT_CN: &str = "Key Attestation CA1";
const SOFTWARE_ROOT_CN: &str = "Android Keystore Software Attestation Root";

pub(crate) fn allowed_digest() -> [u8; 32] {
    Sha256::digest(b"release signing certificate").into()
}

fn transcript() -> Vec<u8> {
    b"kuzz.push.enroll.v1\n{\"v\":1,\"app_profile\":\"kuzz-android\"}".to_vec()
}

// --- DER writer for the KeyDescription extension -------------------------

fn tlv(tag: &[u8], content: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    let len = content.len();
    if len < 0x80 {
        out.push(len as u8);
    } else if len < 0x100 {
        out.extend_from_slice(&[0x81, len as u8]);
    } else {
        out.extend_from_slice(&[0x82, (len >> 8) as u8, len as u8]);
    }
    out.extend_from_slice(content);
    out
}
fn int(v: i64) -> Vec<u8> {
    let bytes = v.to_be_bytes();
    let mut start = 0;
    while start < 7
        && ((bytes[start] == 0 && bytes[start + 1] & 0x80 == 0)
            || (bytes[start] == 0xff && bytes[start + 1] & 0x80 != 0))
    {
        start += 1;
    }
    tlv(&[0x02], &bytes[start..])
}
fn enumerated(v: i64) -> Vec<u8> {
    let mut der = int(v);
    der[0] = 0x0a;
    der
}
fn octets(v: &[u8]) -> Vec<u8> {
    tlv(&[0x04], v)
}
fn seq(parts: &[Vec<u8>]) -> Vec<u8> {
    tlv(&[0x30], &parts.concat())
}
fn set(parts: &[Vec<u8>]) -> Vec<u8> {
    tlv(&[0x31], &parts.concat())
}
const ORIGIN_TAG: [u8; 3] = [0xbf, 0x85, 0x3e];
const APPLICATION_ID_TAG: [u8; 3] = [0xbf, 0x85, 0x45];

#[derive(Clone)]
pub(crate) struct Spec {
    pub(crate) challenge: Vec<u8>,
    pub(crate) attestation_level: i64,
    pub(crate) keymint_level: i64,
    pub(crate) origin: Option<i64>,
    pub(crate) packages: Vec<&'static str>,
    pub(crate) digests: Vec<[u8; 32]>,
    pub(crate) application_id_in_hardware: bool,
    pub(crate) intermediate_expired: bool,
    pub(crate) extension_in_leaf: bool,
    pub(crate) extension_in_intermediate: bool,
    pub(crate) leaf_p384: bool,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            challenge: Sha256::digest(transcript()).to_vec(),
            attestation_level: 1,
            keymint_level: 1,
            origin: Some(0),
            packages: vec![PACKAGE],
            digests: vec![allowed_digest()],
            application_id_in_hardware: false,
            intermediate_expired: false,
            extension_in_leaf: true,
            extension_in_intermediate: false,
            leaf_p384: false,
        }
    }
}

fn key_description(spec: &Spec) -> Vec<u8> {
    let application_id = seq(&[
        set(&spec
            .packages
            .iter()
            .map(|name| seq(&[octets(name.as_bytes()), int(42)]))
            .collect::<Vec<_>>()),
        set(&spec.digests.iter().map(|d| octets(d)).collect::<Vec<_>>()),
    ]);
    let application_id = tlv(&APPLICATION_ID_TAG, &octets(&application_id));
    let mut hardware = Vec::new();
    // Unknown tags (here [1] purpose) are skipped.
    hardware.push(tlv(&[0xa1], &set(&[int(2)])));
    if let Some(origin) = spec.origin {
        hardware.push(tlv(&ORIGIN_TAG, &int(origin)));
    }
    let mut software = Vec::new();
    if spec.application_id_in_hardware {
        hardware.push(application_id);
    } else {
        software.push(application_id);
    }
    seq(&[
        int(300),
        enumerated(spec.attestation_level),
        int(300),
        enumerated(spec.keymint_level),
        octets(&spec.challenge),
        octets(b""),
        seq(&software),
        seq(&hardware),
    ])
}

fn extension(spec: &Spec) -> CustomExtension {
    CustomExtension::from_oid_content(&[1, 3, 6, 1, 4, 1, 11129, 2, 1, 17], key_description(spec))
}

fn params(cn: &str, serial: &[u8], ca: bool) -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name.push(DnType::CommonName, cn);
    params.serial_number = Some(SerialNumber::from(serial.to_vec()));
    if ca {
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    }
    params
}

fn pem(der: &[u8]) -> Vec<u8> {
    let body = STANDARD.encode(der);
    let lines = body
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    format!("-----BEGIN CERTIFICATE-----\n{lines}\n-----END CERTIFICATE-----\n").into_bytes()
}

pub(crate) struct Root {
    params: CertificateParams,
    key: KeyPair,
    pub(crate) pem: Vec<u8>,
}

pub(crate) fn root(cn: &str) -> Root {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384).unwrap();
    let params = params(cn, &[ROOT_SERIAL], true);
    let cert = params.self_signed(&key).unwrap();
    Root {
        pem: pem(cert.der()),
        params,
        key,
    }
}

pub(crate) struct Chain {
    pub(crate) attestation: String,
    pub(crate) key_id: String,
    pub(crate) leaf_key: KeyPair,
}

fn chain(root: &Root, spec: &Spec) -> Chain {
    let leaf_key = KeyPair::generate_for(if spec.leaf_p384 {
        &PKCS_ECDSA_P384_SHA384
    } else {
        &PKCS_ECDSA_P256_SHA256
    })
    .unwrap();
    chain_for_key(root, spec, leaf_key)
}

/// Standard base64 key id of a leaf key, as the app sends it.
pub(crate) fn key_id_of(leaf_key: &KeyPair) -> String {
    STANDARD.encode(Sha256::digest(leaf_key.subject_public_key_info()))
}

/// Chain whose leaf certifies `leaf_key`.
pub(crate) fn chain_for_key(root: &Root, spec: &Spec, leaf_key: KeyPair) -> Chain {
    let intermediate_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut intermediate_params = params("Intermediate", &INTERMEDIATE_SERIAL, true);
    if spec.intermediate_expired {
        intermediate_params.not_after = rcgen::date_time_ymd(2020, 1, 1);
    }
    if spec.extension_in_intermediate {
        intermediate_params.custom_extensions.push(extension(spec));
    }
    let intermediate = intermediate_params
        .signed_by(
            &intermediate_key,
            &Issuer::from_params(&root.params, &root.key),
        )
        .unwrap();
    let mut leaf_params = params("Android Keystore Key", &LEAF_SERIAL, false);
    if spec.extension_in_leaf {
        leaf_params.custom_extensions.push(extension(spec));
    }
    let leaf = leaf_params
        .signed_by(
            &leaf_key,
            &Issuer::from_params(&intermediate_params, &intermediate_key),
        )
        .unwrap();
    let root_der = root.params.self_signed(&root.key).unwrap();
    let der = [leaf.der().as_ref(), intermediate.der(), root_der.der()].concat();
    Chain {
        attestation: STANDARD.encode(der),
        key_id: key_id_of(&leaf_key),
        leaf_key,
    }
}

// --- Revocation fetcher double ---------------------------------------------

#[derive(Default)]
pub(crate) struct Fetcher {
    responses: Mutex<VecDeque<Result<Vec<u8>, ()>>>,
    calls: Mutex<usize>,
}

#[async_trait]
impl StatusListFetcher for Fetcher {
    async fn fetch(&self) -> Result<Vec<u8>, ()> {
        *self.calls.lock().unwrap() += 1;
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(br#"{"entries":{}}"#.to_vec()))
    }
}

impl Fetcher {
    fn push(&self, response: Result<&str, ()>) {
        self.responses
            .lock()
            .unwrap()
            .push_back(response.map(|body| body.as_bytes().to_vec()));
    }
    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

pub(crate) fn verifier_for(roots_pem: &[u8], fetcher: Arc<Fetcher>) -> AndroidAttestVerifier {
    AndroidAttestVerifier::new(
        PACKAGE,
        vec![allowed_digest()],
        roots_pem,
        Arc::new(RevocationList::new(fetcher)),
    )
    .unwrap()
}

async fn verify(root: &Root, spec: &Spec) -> Result<VerifiedAndroidKey, AndroidAttestError> {
    let chain = chain(root, spec);
    verifier_for(&root.pem, Arc::new(Fetcher::default()))
        .verify_attestation(&chain.attestation, &chain.key_id, &transcript(), T0)
        .await
}

#[tokio::test]
async fn hardware_chain_for_the_configured_app_is_accepted() {
    let root = root(HARDWARE_ROOT_CN);
    let chain = chain(&root, &Spec::default());
    let verified = verifier_for(&root.pem, Arc::new(Fetcher::default()))
        .verify_attestation(&chain.attestation, &chain.key_id, &transcript(), T0)
        .await
        .expect("accepted");
    assert_eq!(
        verified.public_key,
        chain.leaf_key.subject_public_key_info()
    );
    assert_eq!(verified.public_key.len(), 91);
    assert_eq!(
        verified.key_id,
        Sha256::digest(&verified.public_key).to_vec()
    );

    for spec in [
        Spec {
            attestation_level: 2,
            keymint_level: 2,
            ..Spec::default()
        },
        Spec {
            application_id_in_hardware: true,
            ..Spec::default()
        },
        Spec {
            packages: vec!["com.example.shared", PACKAGE],
            ..Spec::default()
        },
    ] {
        assert!(verify(&root, &spec).await.is_ok());
    }
}

#[tokio::test]
async fn transcript_package_and_signing_digest_are_bound() {
    let root = root(HARDWARE_ROOT_CN);
    let other: [u8; 32] = Sha256::digest(b"debug signing certificate").into();
    for (spec, expected) in [
        (
            Spec {
                challenge: Sha256::digest(b"another transcript").to_vec(),
                ..Spec::default()
            },
            AndroidAttestError::Challenge,
        ),
        (
            Spec {
                packages: vec!["app.kuzz.mobile.debug"],
                ..Spec::default()
            },
            AndroidAttestError::Package,
        ),
        (
            Spec {
                digests: vec![other],
                ..Spec::default()
            },
            AndroidAttestError::SigningDigest,
        ),
        (
            Spec {
                digests: vec![allowed_digest(), other],
                ..Spec::default()
            },
            AndroidAttestError::SigningDigest,
        ),
        (
            Spec {
                digests: vec![],
                ..Spec::default()
            },
            AndroidAttestError::SigningDigest,
        ),
    ] {
        assert_eq!(verify(&root, &spec).await, Err(expected));
    }
}

#[tokio::test]
async fn software_attestation_is_refused() {
    let root = root(HARDWARE_ROOT_CN);
    for (spec, expected) in [
        (
            Spec {
                attestation_level: 0,
                ..Spec::default()
            },
            AndroidAttestError::SecurityLevel,
        ),
        (
            Spec {
                keymint_level: 0,
                ..Spec::default()
            },
            AndroidAttestError::SecurityLevel,
        ),
        (
            Spec {
                origin: Some(2),
                ..Spec::default()
            },
            AndroidAttestError::Origin,
        ),
        (
            Spec {
                origin: None,
                ..Spec::default()
            },
            AndroidAttestError::Origin,
        ),
    ] {
        assert_eq!(verify(&root, &spec).await, Err(expected));
    }

    // A software root is refused in configuration and in a presented chain,
    // even when the presented chain is otherwise valid.
    let software = self::root(SOFTWARE_ROOT_CN);
    assert!(matches!(
        AndroidAttestVerifier::new(
            PACKAGE,
            vec![allowed_digest()],
            &software.pem,
            Arc::new(RevocationList::new(Arc::new(Fetcher::default()))),
        ),
        Err(AndroidConfigError::SoftwareRoot)
    ));
    let chain = chain(&software, &Spec::default());
    let trusted = [root.pem.clone(), software_spki_trick(&software)].concat();
    assert_eq!(
        verifier_for(&trusted, Arc::new(Fetcher::default()))
            .verify_attestation(&chain.attestation, &chain.key_id, &transcript(), T0)
            .await,
        Err(AndroidAttestError::SoftwareRoot)
    );
}

/// A hardware-named root that reuses the software root's key, so the
/// software chain's root SPKI is in the trusted set and only the name check
/// can refuse it.
fn software_spki_trick(software: &Root) -> Vec<u8> {
    let params = params(HARDWARE_ROOT_CN, &[0x77], true);
    pem(params.self_signed(&software.key).unwrap().der())
}

#[tokio::test]
async fn chain_must_end_in_a_configured_root_and_verify() {
    let trusted = root(HARDWARE_ROOT_CN);
    let other = root(HARDWARE_ROOT_CN);
    let chain_from_other = chain(&other, &Spec::default());
    let verifier = verifier_for(&trusted.pem, Arc::new(Fetcher::default()));
    assert_eq!(
        verifier
            .verify_attestation(
                &chain_from_other.attestation,
                &chain_from_other.key_id,
                &transcript(),
                T0
            )
            .await,
        Err(AndroidAttestError::UntrustedRoot)
    );

    // Splice: leaf and intermediate from one chain, root from another.
    let good = chain(&trusted, &Spec::default());
    let good_der = STANDARD.decode(&good.attestation).unwrap();
    let other_der = STANDARD.decode(&chain_from_other.attestation).unwrap();
    let good_certs = split_chain(&good_der);
    let other_certs = split_chain(&other_der);
    let spliced = [good_certs[0], other_certs[1], good_certs[2]].concat();
    assert_eq!(
        verifier
            .verify_attestation(&STANDARD.encode(spliced), &good.key_id, &transcript(), T0)
            .await,
        Err(AndroidAttestError::Chain)
    );

    let leaf_only = good_certs[0].to_vec();
    assert_eq!(
        verifier
            .verify_attestation(&STANDARD.encode(leaf_only), &good.key_id, &transcript(), T0)
            .await,
        Err(AndroidAttestError::Malformed)
    );
    assert_eq!(
        verifier
            .verify_attestation("not base64!", &good.key_id, &transcript(), T0)
            .await,
        Err(AndroidAttestError::Malformed)
    );
    assert_eq!(
        verify(
            &trusted,
            &Spec {
                intermediate_expired: true,
                ..Spec::default()
            }
        )
        .await,
        Err(AndroidAttestError::Expired)
    );
}

fn split_chain(mut der: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    while !der.is_empty() {
        let (rest, _) = x509_parser::parse_x509_certificate(der).unwrap();
        out.push(&der[..der.len() - rest.len()]);
        der = rest;
    }
    out
}

#[tokio::test]
async fn extension_placement_key_type_and_key_id_are_enforced() {
    let root = root(HARDWARE_ROOT_CN);
    for (spec, expected) in [
        (
            Spec {
                extension_in_leaf: false,
                ..Spec::default()
            },
            AndroidAttestError::Extension,
        ),
        (
            Spec {
                extension_in_intermediate: true,
                ..Spec::default()
            },
            AndroidAttestError::Extension,
        ),
        (
            Spec {
                leaf_p384: true,
                ..Spec::default()
            },
            AndroidAttestError::KeyType,
        ),
    ] {
        assert_eq!(verify(&root, &spec).await, Err(expected));
    }
    let chain = chain(&root, &Spec::default());
    assert_eq!(
        verifier_for(&root.pem, Arc::new(Fetcher::default()))
            .verify_attestation(
                &chain.attestation,
                &STANDARD.encode([0u8; 32]),
                &transcript(),
                T0
            )
            .await,
        Err(AndroidAttestError::KeyId)
    );
}

#[tokio::test]
async fn revoked_or_suspended_certificates_are_refused() {
    let root = root(HARDWARE_ROOT_CN);
    let chain = chain(&root, &Spec::default());
    for listed in [
        r#"{"entries":{"a0b":{"status":"REVOKED","reason":"KEY_COMPROMISE"}}}"#,
        r#"{"entries":{"0C0D":{"status":"SUSPENDED"}}}"#,
        r#"{"entries":{"51":{"status":"REVOKED"}}}"#,
    ] {
        let fetcher = Arc::new(Fetcher::default());
        fetcher.push(Ok(listed));
        assert_eq!(
            verifier_for(&root.pem, fetcher)
                .verify_attestation(&chain.attestation, &chain.key_id, &transcript(), T0)
                .await,
            Err(AndroidAttestError::Revoked),
            "{listed}"
        );
    }
}

#[tokio::test]
async fn revocation_list_fails_open_only_while_under_72_hours_old() {
    let root = root(HARDWARE_ROOT_CN);
    let chain = chain(&root, &Spec::default());
    let fetcher = Arc::new(Fetcher::default());
    let verifier = verifier_for(&root.pem, fetcher.clone());
    let signed = transcript();
    let at = |now| verifier.verify_attestation(&chain.attestation, &chain.key_id, &signed, now);

    assert!(at(T0).await.is_ok());
    assert!(at(T0 + 3_600).await.is_ok());
    assert_eq!(fetcher.calls(), 1, "cached for 24 h");

    // Refresh due; the fetch fails, the 25 h old list is still used.
    fetcher.push(Err(()));
    assert!(at(T0 + 25 * 3_600).await.is_ok());
    assert_eq!(fetcher.calls(), 2);
    // Within the retry back-off no new fetch is made.
    assert!(at(T0 + 25 * 3_600 + 60).await.is_ok());
    assert_eq!(fetcher.calls(), 2);

    // At 72 h the cached list is too old: refuse when the refresh fails.
    fetcher.push(Err(()));
    assert_eq!(
        at(T0 + 72 * 3_600).await,
        Err(AndroidAttestError::RevocationUnavailable)
    );
    // A later successful refresh restores enrollment.
    assert!(at(T0 + 72 * 3_600 + STATUS_LIST_RETRY_SECONDS)
        .await
        .is_ok());
    assert_eq!(fetcher.calls(), 4);

    // No list at all: refuse.
    let empty = Arc::new(Fetcher::default());
    empty.push(Err(()));
    assert_eq!(
        verifier_for(&root.pem, empty)
            .verify_attestation(&chain.attestation, &chain.key_id, &transcript(), T0)
            .await,
        Err(AndroidAttestError::RevocationUnavailable)
    );
    // An unparseable list is a failed fetch.
    let garbage = Arc::new(Fetcher::default());
    garbage.push(Ok("<html>"));
    assert_eq!(
        verifier_for(&root.pem, garbage)
            .verify_attestation(&chain.attestation, &chain.key_id, &transcript(), T0)
            .await,
        Err(AndroidAttestError::RevocationUnavailable)
    );
}

#[tokio::test]
async fn assertions_verify_only_for_the_attested_key_and_exact_transcript() {
    let root = root(HARDWARE_ROOT_CN);
    let chain = chain(&root, &Spec::default());
    let spki = chain.leaf_key.subject_public_key_info();
    let message = b"kuzz.push.delegate.v1\n{\"v\":1}";
    let signature = STANDARD.encode(chain.leaf_key.sign(message).unwrap());
    assert_eq!(verify_assertion(&spki, &signature, message), Ok(()));
    assert_eq!(
        verify_assertion(&spki, &signature, b"kuzz.push.delegate.v1\n{\"v\":2}"),
        Err(AndroidAttestError::Signature)
    );
    let other = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    assert_eq!(
        verify_assertion(&other.subject_public_key_info(), &signature, message),
        Err(AndroidAttestError::Signature)
    );
    assert_eq!(
        verify_assertion(&spki, "%%%", message),
        Err(AndroidAttestError::Signature)
    );
}

#[test]
fn configuration_requires_roots_package_and_digests() {
    let root = root(HARDWARE_ROOT_CN);
    let revocation = || Arc::new(RevocationList::new(Arc::new(Fetcher::default())));
    assert!(matches!(
        AndroidAttestVerifier::new(PACKAGE, vec![allowed_digest()], b"", revocation()),
        Err(AndroidConfigError::NoRoots)
    ));
    assert!(matches!(
        AndroidAttestVerifier::new(PACKAGE, vec![], &root.pem, revocation()),
        Err(AndroidConfigError::Digests)
    ));
    assert!(matches!(
        AndroidAttestVerifier::new("", vec![allowed_digest()], &root.pem, revocation()),
        Err(AndroidConfigError::Package)
    ));
    // An intermediate (not self-signed) is not a root.
    let intermediate_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let intermediate = params("Intermediate", &[1], true)
        .signed_by(
            &intermediate_key,
            &Issuer::from_params(&root.params, &root.key),
        )
        .unwrap();
    assert!(matches!(
        AndroidAttestVerifier::new(
            PACKAGE,
            vec![allowed_digest()],
            &pem(intermediate.der()),
            revocation()
        ),
        Err(AndroidConfigError::NotSelfSigned)
    ));
}

#[tokio::test]
async fn http_status_list_fetch_is_bounded() {
    use axum::{routing::get, Router};
    let app = Router::new()
        .route("/ok", get(|| async { r#"{"entries":{}}"# }))
        .route(
            "/huge",
            get(|| async { "x".repeat(MAX_STATUS_LIST_BYTES + 1) }),
        )
        .route(
            "/down",
            get(|| async { (axum::http::StatusCode::SERVICE_UNAVAILABLE, "") }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let fetch = |path: &str| {
        let fetcher = HttpStatusListFetcher::with_url(format!("{base}{path}")).unwrap();
        async move { fetcher.fetch().await }
    };
    assert_eq!(fetch("/ok").await, Ok(br#"{"entries":{}}"#.to_vec()));
    assert_eq!(fetch("/huge").await, Err(()));
    assert_eq!(fetch("/down").await, Err(()));
}
