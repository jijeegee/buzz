//! Android Key Attestation for `kuzz-android` installations.
//!
//! Enrollment carries the certificate chain of a fresh Keystore key whose
//! attestation challenge is SHA-256 of the enrollment transcript. The gateway
//! accepts the key only when:
//! - every certificate is signed by the next and the chain ends in a
//!   configured Google hardware attestation root (the software root is
//!   refused);
//! - no certificate is on Google's revocation status list (the list is
//!   cached for 24 h; when a refresh fails, a list under 72 h old is still
//!   used, an older or missing list refuses enrollment);
//! - attestation and KeyMint security levels are TEE or StrongBox;
//! - the key was generated inside the secure hardware (origin GENERATED);
//! - the attested application is the configured package, signed only by
//!   configured certificate digests;
//! - the key is EC P-256.
//!
//! Later signed mutations are ECDSA P-256 SHA-256 signatures by that key over
//! the exact transcript bytes.

use std::{collections::HashSet, sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};
use thiserror::Error;
use x509_parser::{certificate::X509Certificate, prelude::FromDer, time::ASN1Time};

use crate::model::MAX_APP_ATTESTATION_BYTES;

/// Android Key Attestation extension (KeyDescription).
pub const KEY_DESCRIPTION_OID: &str = "1.3.6.1.4.1.11129.2.1.17";
/// Google's attestation revocation status list.
pub const GOOGLE_STATUS_LIST_URL: &str = "https://android.googleapis.com/attestation/status";
/// Revocation list freshness before a refresh is attempted.
pub const STATUS_LIST_REFRESH_SECONDS: i64 = 24 * 60 * 60;
/// Oldest cached revocation list still used when a refresh fails.
pub const STATUS_LIST_MAX_AGE_SECONDS: i64 = 72 * 60 * 60;
/// Wait after a failed refresh before the next attempt.
pub const STATUS_LIST_RETRY_SECONDS: i64 = 300;
const MAX_STATUS_LIST_BYTES: usize = 8 * 1024 * 1024;
const STATUS_LIST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CHAIN_LENGTH: usize = 8;
const TAG_ORIGIN: u32 = 702;
const TAG_ATTESTATION_APPLICATION_ID: u32 = 709;
const ORIGIN_GENERATED: i64 = 0;
const SECURITY_LEVEL_TEE: i64 = 1;
const SECURITY_LEVEL_STRONGBOX: i64 = 2;
/// DER prefix of every uncompressed EC P-256 SubjectPublicKeyInfo.
const P256_SPKI_PREFIX: [u8; 27] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04,
];
const P256_SPKI_LEN: usize = 91;

/// Why an attestation or assertion was refused. Variants carry no device or
/// credential data, so they are safe to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AndroidAttestError {
    #[error("malformed attestation")]
    Malformed,
    #[error("certificate chain does not verify")]
    Chain,
    #[error("chain does not end in a trusted hardware root")]
    UntrustedRoot,
    #[error("software attestation root")]
    SoftwareRoot,
    #[error("certificate outside its validity period")]
    Expired,
    #[error("attestation extension missing, duplicated or misplaced")]
    Extension,
    #[error("attestation or KeyMint security level is not TEE or StrongBox")]
    SecurityLevel,
    #[error("attestation challenge does not match the transcript")]
    Challenge,
    #[error("key was not generated in secure hardware")]
    Origin,
    #[error("attested package is not the configured application")]
    Package,
    #[error("attested signing certificate is not allowed")]
    SigningDigest,
    #[error("attested key is not EC P-256")]
    KeyType,
    #[error("key id does not match the attested key")]
    KeyId,
    #[error("certificate revoked")]
    Revoked,
    #[error("revocation status unavailable")]
    RevocationUnavailable,
    #[error("signature does not verify")]
    Signature,
}

/// Configuration problem found at startup. Carries no key material.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AndroidConfigError {
    #[error("no Google hardware attestation root in the root bundle")]
    NoRoots,
    #[error("unparseable certificate in the root bundle")]
    Root,
    #[error("the root bundle contains a software attestation root")]
    SoftwareRoot,
    #[error("the root bundle contains a certificate that is not self-signed")]
    NotSelfSigned,
    #[error("package name is empty or too long")]
    Package,
    #[error("signing digest allowlist is empty or malformed")]
    Digests,
}

/// Attested key admitted for one installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedAndroidKey {
    /// SHA-256 of `public_key`; stored as the installation key id.
    pub key_id: Vec<u8>,
    /// SubjectPublicKeyInfo DER of the attested EC P-256 key.
    pub public_key: Vec<u8>,
}

/// Source of Google's revocation status list body.
#[async_trait]
pub trait StatusListFetcher: Send + Sync {
    /// Fetch the raw JSON body, or fail.
    async fn fetch(&self) -> Result<Vec<u8>, ()>;
}

/// HTTPS fetcher for [`GOOGLE_STATUS_LIST_URL`] with a body cap and timeout.
pub struct HttpStatusListFetcher {
    client: reqwest::Client,
    url: String,
}

impl HttpStatusListFetcher {
    /// Fetcher for Google's production status list.
    pub fn google() -> Result<Self, reqwest::Error> {
        Self::with_url(GOOGLE_STATUS_LIST_URL.to_owned())
    }

    fn with_url(url: String) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .connect_timeout(STATUS_LIST_TIMEOUT)
                .timeout(STATUS_LIST_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            url,
        })
    }
}

#[async_trait]
impl StatusListFetcher for HttpStatusListFetcher {
    async fn fetch(&self) -> Result<Vec<u8>, ()> {
        let mut response = self.client.get(&self.url).send().await.map_err(|_| ())?;
        if !response.status().is_success() {
            return Err(());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
            if body.len() + chunk.len() > MAX_STATUS_LIST_BYTES {
                return Err(());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

#[derive(serde::Deserialize)]
struct StatusList {
    entries: std::collections::HashMap<String, serde_json::Value>,
}

/// Parse a status list into the set of listed serials (lowercase hex). Both
/// REVOKED and SUSPENDED entries refuse enrollment.
fn parse_status_list(body: &[u8]) -> Result<HashSet<String>, ()> {
    let list: StatusList = serde_json::from_slice(body).map_err(|_| ())?;
    Ok(list
        .entries
        .into_keys()
        .map(|serial| serial.trim_start_matches('0').to_ascii_lowercase())
        .collect())
}

#[derive(Default)]
struct StatusState {
    listed: Option<(Arc<HashSet<String>>, i64)>,
    retry_after: i64,
}

/// Cached revocation status list. One refresh runs at a time.
pub struct RevocationList {
    fetcher: Arc<dyn StatusListFetcher>,
    state: tokio::sync::Mutex<StatusState>,
}

impl RevocationList {
    /// Empty cache; the first check fetches.
    pub fn new(fetcher: Arc<dyn StatusListFetcher>) -> Self {
        Self {
            fetcher,
            state: tokio::sync::Mutex::new(StatusState::default()),
        }
    }

    async fn current(&self, now: i64) -> Result<Arc<HashSet<String>>, AndroidAttestError> {
        let mut state = self.state.lock().await;
        if let Some((listed, fetched_at)) = &state.listed {
            if now.saturating_sub(*fetched_at) < STATUS_LIST_REFRESH_SECONDS {
                return Ok(Arc::clone(listed));
            }
        }
        if now >= state.retry_after {
            match self
                .fetcher
                .fetch()
                .await
                .and_then(|b| parse_status_list(&b))
            {
                Ok(listed) => {
                    let listed = Arc::new(listed);
                    state.listed = Some((Arc::clone(&listed), now));
                    state.retry_after = 0;
                    return Ok(listed);
                }
                Err(()) => {
                    state.retry_after = now.saturating_add(STATUS_LIST_RETRY_SECONDS);
                }
            }
        }
        match &state.listed {
            Some((listed, fetched_at))
                if now.saturating_sub(*fetched_at) < STATUS_LIST_MAX_AGE_SECONDS =>
            {
                tracing::warn!(
                    age_seconds = now.saturating_sub(*fetched_at),
                    "Android attestation status list refresh failed; using the cached list"
                );
                Ok(Arc::clone(listed))
            }
            _ => {
                tracing::warn!(
                    "Android attestation status list unavailable or older than 72 h; refusing"
                );
                Err(AndroidAttestError::RevocationUnavailable)
            }
        }
    }

    /// Refuse when any serial is listed or no usable list exists.
    pub async fn check(&self, serials: &[String], now: i64) -> Result<(), AndroidAttestError> {
        let listed = self.current(now).await?;
        if serials.iter().any(|serial| listed.contains(serial)) {
            return Err(AndroidAttestError::Revoked);
        }
        Ok(())
    }
}

/// Verifier bound to one Android application identity.
pub struct AndroidAttestVerifier {
    package: Vec<u8>,
    signing_digests: HashSet<[u8; 32]>,
    root_spkis: Vec<Vec<u8>>,
    revocation: Arc<RevocationList>,
}

impl AndroidAttestVerifier {
    /// Build a verifier. `roots_pem` is a bundle of Google hardware
    /// attestation root certificates; every one must be self-signed and none
    /// may be a software attestation root.
    pub fn new(
        package: &str,
        signing_digests: Vec<[u8; 32]>,
        roots_pem: &[u8],
        revocation: Arc<RevocationList>,
    ) -> Result<Self, AndroidConfigError> {
        if package.is_empty() || package.len() > 255 {
            return Err(AndroidConfigError::Package);
        }
        if signing_digests.is_empty() {
            return Err(AndroidConfigError::Digests);
        }
        let mut root_spkis = Vec::new();
        for pem in x509_parser::pem::Pem::iter_from_buffer(roots_pem) {
            let pem = pem.map_err(|_| AndroidConfigError::Root)?;
            let cert = pem.parse_x509().map_err(|_| AndroidConfigError::Root)?;
            if is_software_root(&cert) {
                return Err(AndroidConfigError::SoftwareRoot);
            }
            if cert.verify_signature(None).is_err() {
                return Err(AndroidConfigError::NotSelfSigned);
            }
            root_spkis.push(cert.public_key().raw.to_vec());
        }
        if root_spkis.is_empty() {
            return Err(AndroidConfigError::NoRoots);
        }
        Ok(Self {
            package: package.as_bytes().to_vec(),
            signing_digests: signing_digests.into_iter().collect(),
            root_spkis,
            revocation,
        })
    }

    /// Verify an enrollment attestation for `transcript`, returning the key to
    /// store. `attestation` is standard base64 of the concatenated DER chain,
    /// leaf first; `key_id` is standard base64 of SHA-256 of the leaf SPKI.
    pub async fn verify_attestation(
        &self,
        attestation: &str,
        key_id: &str,
        transcript: &[u8],
        now: i64,
    ) -> Result<VerifiedAndroidKey, AndroidAttestError> {
        if attestation.len() > MAX_APP_ATTESTATION_BYTES.div_ceil(3) * 4 {
            return Err(AndroidAttestError::Malformed);
        }
        let der = STANDARD
            .decode(attestation)
            .map_err(|_| AndroidAttestError::Malformed)?;
        let claimed_key_id = STANDARD
            .decode(key_id)
            .map_err(|_| AndroidAttestError::Malformed)?;
        let chain = parse_chain(&der)?;
        self.verify_chain(&chain, now)?;
        let leaf = &chain[0];
        let description = leaf_key_description(&chain)?;
        let key = parse_key_description(description)?;
        let levels = [key.attestation_security_level, key.keymint_security_level];
        if !levels
            .iter()
            .all(|level| matches!(*level, SECURITY_LEVEL_TEE | SECURITY_LEVEL_STRONGBOX))
        {
            return Err(AndroidAttestError::SecurityLevel);
        }
        let expected: [u8; 32] = Sha256::digest(transcript).into();
        if key.challenge != expected {
            return Err(AndroidAttestError::Challenge);
        }
        if key.hardware.origin != Some(ORIGIN_GENERATED) {
            return Err(AndroidAttestError::Origin);
        }
        let application = key
            .hardware
            .application_id
            .or(key.software.application_id)
            .ok_or(AndroidAttestError::Package)?;
        self.check_application(application)?;
        let public_key = leaf.public_key().raw.to_vec();
        if !is_p256_spki(&public_key) {
            return Err(AndroidAttestError::KeyType);
        }
        let key_id: [u8; 32] = Sha256::digest(&public_key).into();
        if claimed_key_id != key_id {
            return Err(AndroidAttestError::KeyId);
        }
        let serials = chain
            .iter()
            .map(|cert| format!("{:x}", cert.serial))
            .collect::<Vec<_>>();
        self.revocation.check(&serials, now).await?;
        Ok(VerifiedAndroidKey {
            key_id: key_id.to_vec(),
            public_key,
        })
    }

    fn verify_chain(
        &self,
        chain: &[X509Certificate<'_>],
        now: i64,
    ) -> Result<(), AndroidAttestError> {
        let now = ASN1Time::from_timestamp(now).map_err(|_| AndroidAttestError::Malformed)?;
        for pair in chain.windows(2) {
            let (child, parent) = (&pair[0], &pair[1]);
            if child.issuer().as_raw() != parent.subject().as_raw()
                || child.verify_signature(Some(parent.public_key())).is_err()
                || !parent.is_ca()
            {
                return Err(AndroidAttestError::Chain);
            }
        }
        let root = chain.last().ok_or(AndroidAttestError::Malformed)?;
        if is_software_root(root) {
            return Err(AndroidAttestError::SoftwareRoot);
        }
        if root.verify_signature(None).is_err()
            || !self
                .root_spkis
                .iter()
                .any(|trusted| trusted.as_slice() == root.public_key().raw)
        {
            return Err(AndroidAttestError::UntrustedRoot);
        }
        // Leaf validity is device-defined (often epoch or far future) and is
        // not checked; issuing certificates must be current.
        if chain[1..]
            .iter()
            .any(|cert| !cert.validity().is_valid_at(now))
        {
            return Err(AndroidAttestError::Expired);
        }
        Ok(())
    }

    fn check_application(&self, encoded: &[u8]) -> Result<(), AndroidAttestError> {
        let (packages, digests) = parse_application_id(encoded)?;
        if !packages.contains(&self.package.as_slice()) {
            return Err(AndroidAttestError::Package);
        }
        if digests.is_empty()
            || !digests.iter().all(|digest| {
                <[u8; 32]>::try_from(*digest).is_ok_and(|d| self.signing_digests.contains(&d))
            })
        {
            return Err(AndroidAttestError::SigningDigest);
        }
        Ok(())
    }
}

/// Verify an Android installation assertion: standard base64 of a DER ECDSA
/// P-256 SHA-256 signature by the attested key over `transcript`.
pub fn verify_assertion(
    public_key_spki: &[u8],
    assertion: &str,
    transcript: &[u8],
) -> Result<(), AndroidAttestError> {
    if !is_p256_spki(public_key_spki) || assertion.len() > 128 {
        return Err(AndroidAttestError::Signature);
    }
    let signature = STANDARD
        .decode(assertion)
        .map_err(|_| AndroidAttestError::Signature)?;
    let (_, spki) = x509_parser::x509::SubjectPublicKeyInfo::from_der(public_key_spki)
        .map_err(|_| AndroidAttestError::Signature)?;
    aws_lc_rs::signature::UnparsedPublicKey::new(
        &aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1,
        spki.subject_public_key.data.as_ref(),
    )
    .verify(transcript, &signature)
    .map_err(|_| AndroidAttestError::Signature)
}

fn is_p256_spki(spki: &[u8]) -> bool {
    spki.len() == P256_SPKI_LEN && spki.starts_with(&P256_SPKI_PREFIX)
}

fn is_software_root(cert: &X509Certificate<'_>) -> bool {
    cert.subject().iter_common_name().any(|cn| {
        cn.as_str()
            .is_ok_and(|name| name.contains("Software Attestation"))
    })
}

fn parse_chain(mut der: &[u8]) -> Result<Vec<X509Certificate<'_>>, AndroidAttestError> {
    let mut chain = Vec::new();
    while !der.is_empty() {
        if chain.len() == MAX_CHAIN_LENGTH {
            return Err(AndroidAttestError::Malformed);
        }
        let (rest, cert) =
            x509_parser::parse_x509_certificate(der).map_err(|_| AndroidAttestError::Malformed)?;
        chain.push(cert);
        der = rest;
    }
    if chain.len() < 2 {
        return Err(AndroidAttestError::Malformed);
    }
    Ok(chain)
}

/// The KeyDescription extension must be in the leaf exactly once and in no
/// issuing certificate.
fn leaf_key_description<'a>(
    chain: &'a [X509Certificate<'a>],
) -> Result<&'a [u8], AndroidAttestError> {
    let matching = |cert: &'a X509Certificate<'a>| {
        cert.extensions()
            .iter()
            .filter(|ext| ext.oid.to_id_string() == KEY_DESCRIPTION_OID)
            .map(|ext| ext.value)
            .collect::<Vec<_>>()
    };
    if chain[1..].iter().any(|cert| !matching(cert).is_empty()) {
        return Err(AndroidAttestError::Extension);
    }
    match matching(&chain[0]).as_slice() {
        [value] => Ok(value),
        _ => Err(AndroidAttestError::Extension),
    }
}

// --- Minimal DER reader for KeyDescription -------------------------------

const CLASS_UNIVERSAL: u8 = 0;
const CLASS_CONTEXT: u8 = 2;
const TAG_INTEGER: u32 = 2;
const TAG_OCTET_STRING: u32 = 4;
const TAG_ENUMERATED: u32 = 10;
const TAG_SEQUENCE: u32 = 16;
const TAG_SET: u32 = 17;

struct Tlv<'a> {
    class: u8,
    constructed: bool,
    number: u32,
    content: &'a [u8],
}

/// Read one definite-length TLV. Returns it and the remaining input.
fn read_tlv(input: &[u8]) -> Result<(Tlv<'_>, &[u8]), AndroidAttestError> {
    let malformed = AndroidAttestError::Malformed;
    let (&first, mut rest) = input.split_first().ok_or(malformed)?;
    let class = first >> 6;
    let constructed = first & 0x20 != 0;
    let mut number = u32::from(first & 0x1f);
    if number == 0x1f {
        number = 0;
        for i in 0.. {
            let (&b, tail) = rest.split_first().ok_or(malformed)?;
            rest = tail;
            if i == 4 || (i == 0 && b == 0x80) {
                return Err(malformed);
            }
            number = (number << 7) | u32::from(b & 0x7f);
            if b & 0x80 == 0 {
                break;
            }
        }
    }
    let (&len_byte, tail) = rest.split_first().ok_or(malformed)?;
    rest = tail;
    let len = if len_byte < 0x80 {
        usize::from(len_byte)
    } else {
        let count = usize::from(len_byte & 0x7f);
        if count == 0 || count > 4 || rest.len() < count {
            return Err(malformed);
        }
        let (bytes, tail) = rest.split_at(count);
        rest = tail;
        bytes
            .iter()
            .fold(0usize, |acc, b| (acc << 8) | usize::from(*b))
    };
    if rest.len() < len {
        return Err(malformed);
    }
    let (content, rest) = rest.split_at(len);
    Ok((
        Tlv {
            class,
            constructed,
            number,
            content,
        },
        rest,
    ))
}

fn expect(input: &[u8], class: u8, number: u32) -> Result<(&[u8], &[u8]), AndroidAttestError> {
    let (tlv, rest) = read_tlv(input)?;
    if tlv.class != class || tlv.number != number {
        return Err(AndroidAttestError::Malformed);
    }
    Ok((tlv.content, rest))
}

fn small_int(content: &[u8]) -> Result<i64, AndroidAttestError> {
    if content.is_empty() || content.len() > 8 {
        return Err(AndroidAttestError::Malformed);
    }
    let negative = content[0] & 0x80 != 0;
    let start: i64 = if negative { -1 } else { 0 };
    Ok(content
        .iter()
        .fold(start, |acc, b| (acc << 8) | i64::from(*b)))
}

#[derive(Default)]
struct AuthorizationList<'a> {
    origin: Option<i64>,
    application_id: Option<&'a [u8]>,
}

struct KeyDescription<'a> {
    attestation_security_level: i64,
    keymint_security_level: i64,
    challenge: &'a [u8],
    software: AuthorizationList<'a>,
    hardware: AuthorizationList<'a>,
}

fn parse_key_description(der: &[u8]) -> Result<KeyDescription<'_>, AndroidAttestError> {
    let (body, trailing) = expect(der, CLASS_UNIVERSAL, TAG_SEQUENCE)?;
    if !trailing.is_empty() {
        return Err(AndroidAttestError::Malformed);
    }
    let (_, rest) = expect(body, CLASS_UNIVERSAL, TAG_INTEGER)?;
    let (attestation_level, rest) = expect(rest, CLASS_UNIVERSAL, TAG_ENUMERATED)?;
    let (_, rest) = expect(rest, CLASS_UNIVERSAL, TAG_INTEGER)?;
    let (keymint_level, rest) = expect(rest, CLASS_UNIVERSAL, TAG_ENUMERATED)?;
    let (challenge, rest) = expect(rest, CLASS_UNIVERSAL, TAG_OCTET_STRING)?;
    let (_, rest) = expect(rest, CLASS_UNIVERSAL, TAG_OCTET_STRING)?;
    let (software, rest) = expect(rest, CLASS_UNIVERSAL, TAG_SEQUENCE)?;
    let (hardware, rest) = expect(rest, CLASS_UNIVERSAL, TAG_SEQUENCE)?;
    if !rest.is_empty() {
        return Err(AndroidAttestError::Malformed);
    }
    Ok(KeyDescription {
        attestation_security_level: small_int(attestation_level)?,
        keymint_security_level: small_int(keymint_level)?,
        challenge,
        software: parse_authorization_list(software)?,
        hardware: parse_authorization_list(hardware)?,
    })
}

fn parse_authorization_list(mut input: &[u8]) -> Result<AuthorizationList<'_>, AndroidAttestError> {
    let mut list = AuthorizationList::default();
    let mut seen = HashSet::new();
    while !input.is_empty() {
        let (tagged, rest) = read_tlv(input)?;
        input = rest;
        if tagged.class != CLASS_CONTEXT || !tagged.constructed || !seen.insert(tagged.number) {
            return Err(AndroidAttestError::Malformed);
        }
        match tagged.number {
            TAG_ORIGIN => {
                let (value, rest) = expect(tagged.content, CLASS_UNIVERSAL, TAG_INTEGER)?;
                if !rest.is_empty() {
                    return Err(AndroidAttestError::Malformed);
                }
                list.origin = Some(small_int(value)?);
            }
            TAG_ATTESTATION_APPLICATION_ID => {
                let (value, rest) = expect(tagged.content, CLASS_UNIVERSAL, TAG_OCTET_STRING)?;
                if !rest.is_empty() {
                    return Err(AndroidAttestError::Malformed);
                }
                list.application_id = Some(value);
            }
            _ => {}
        }
    }
    Ok(list)
}

type ApplicationId<'a> = (Vec<&'a [u8]>, Vec<&'a [u8]>);

/// AttestationApplicationId ::= SEQUENCE {
///   package_infos SET OF SEQUENCE { package_name OCTET STRING, version INTEGER },
///   signature_digests SET OF OCTET STRING }
fn parse_application_id(der: &[u8]) -> Result<ApplicationId<'_>, AndroidAttestError> {
    let (body, trailing) = expect(der, CLASS_UNIVERSAL, TAG_SEQUENCE)?;
    if !trailing.is_empty() {
        return Err(AndroidAttestError::Malformed);
    }
    let (mut infos, rest) = expect(body, CLASS_UNIVERSAL, TAG_SET)?;
    let (mut digest_set, rest) = expect(rest, CLASS_UNIVERSAL, TAG_SET)?;
    if !rest.is_empty() {
        return Err(AndroidAttestError::Malformed);
    }
    let mut packages = Vec::new();
    while !infos.is_empty() {
        let (info, rest) = expect(infos, CLASS_UNIVERSAL, TAG_SEQUENCE)?;
        infos = rest;
        let (name, rest) = expect(info, CLASS_UNIVERSAL, TAG_OCTET_STRING)?;
        let (_, rest) = expect(rest, CLASS_UNIVERSAL, TAG_INTEGER)?;
        if !rest.is_empty() {
            return Err(AndroidAttestError::Malformed);
        }
        packages.push(name);
    }
    let mut digests = Vec::new();
    while !digest_set.is_empty() {
        let (digest, rest) = expect(digest_set, CLASS_UNIVERSAL, TAG_OCTET_STRING)?;
        digest_set = rest;
        digests.push(digest);
    }
    Ok((packages, digests))
}

#[cfg(test)]
#[path = "android_attest_tests.rs"]
pub(crate) mod tests;
