//! Per-account user-stream switches (`KUZZ_USER_STREAM`, `_LIVE`, `_GATED`,
//! `KUZZ_STREAM_EPOCH`), read once at startup.
//!
//! Each switch is `off`, `on`, or `allowlist:<hex>,<hex>,…`. An account's mode
//! is the highest one whose set **and every wider set** contain it:
//! `gated` needs GATED ∩ LIVE ∩ STREAM, `live` needs LIVE ∩ STREAM, and
//! `shadow` needs STREAM. With the stream `off`, LIVE and GATED are ignored
//! with a warning (even a malformed one); a narrower set that is not inside a
//! wider one is a warning too, never a refusal, so a rollback that narrows one
//! switch always starts. Empty allowlist entries (`allowlist:`, a trailing
//! comma) are skipped; a list left empty is `off`, with a warning.
//!
//! Refusals (deterministic, also run by `kuzz-relay --check-config`): stream
//! not off with no epoch, an epoch over 64 characters, `KUZZ_MESH=on`,
//! effective GATED with replica routing on, and a malformed value while the
//! stream is not off.
//!
//! The stream is single-process: while it is on, run exactly one relay pod
//! per Redis. Startup logs [`SINGLE_POD_WARNING`]; only `KUZZ_MESH=on` is
//! detectable and refused.

use std::collections::BTreeSet;
use std::fmt;

/// Longest accepted `KUZZ_STREAM_EPOCH` (the clients refuse longer ones).
pub const MAX_EPOCH_CHARS: usize = 64;

/// Logged at startup (and by `--check-config`) while the stream is not off.
/// A second pod would learn a cross-pod join only from a spawned recheck,
/// after the 44100 already reached the client, so a `live`/`gated` stream
/// could miss frames the client counts as covered.
pub const SINGLE_POD_WARNING: &str = "KUZZ_USER_STREAM is on: this requires a single relay pod \
     per Redis (cross-pod joins can leave unannounced gaps in live/gated streams)";

/// One switch: who it covers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Access {
    /// Nobody (unset, empty or `off`).
    #[default]
    Off,
    /// Every account.
    On,
    /// The listed accounts (lowercase 64-hex pubkeys).
    Allowlist(BTreeSet<String>),
}

impl Access {
    /// Parse one switch value. `name` is used in messages only. Empty
    /// allowlist entries are skipped; a list with no entry left is `Off`
    /// and pushes a warning.
    pub fn parse(
        name: &str,
        raw: Option<&str>,
        warnings: &mut Vec<String>,
    ) -> Result<Self, String> {
        let raw = raw.map(str::trim).unwrap_or("");
        if raw.is_empty() || raw.eq_ignore_ascii_case("off") {
            return Ok(Self::Off);
        }
        if raw.eq_ignore_ascii_case("on") {
            return Ok(Self::On);
        }
        let Some(list) = raw
            .strip_prefix("allowlist:")
            .or_else(|| raw.strip_prefix("ALLOWLIST:"))
        else {
            return Err(format!(
                "{name} must be off, on or allowlist:<hex,...>, got {raw:?}"
            ));
        };
        let mut set = BTreeSet::new();
        for entry in list.split(',') {
            let entry = entry.trim().to_ascii_lowercase();
            if entry.is_empty() {
                continue;
            }
            if entry.len() != 64 || !entry.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!(
                    "{name}: malformed allowlist entry {entry:?} (need a 64-character hex pubkey)"
                ));
            }
            set.insert(entry);
        }
        if set.is_empty() {
            warnings.push(format!("{name} is an empty allowlist: treated as off"));
            return Ok(Self::Off);
        }
        Ok(Self::Allowlist(set))
    }

    fn contains(&self, pubkey_hex: &str) -> bool {
        match self {
            Self::Off => false,
            Self::On => true,
            Self::Allowlist(set) => set.contains(pubkey_hex),
        }
    }

    fn intersect(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Off, _) | (_, Self::Off) => Self::Off,
            (Self::On, x) | (x, Self::On) => x.clone(),
            (Self::Allowlist(a), Self::Allowlist(b)) => {
                let both: BTreeSet<String> = a.intersection(b).cloned().collect();
                if both.is_empty() {
                    Self::Off
                } else {
                    Self::Allowlist(both)
                }
            }
        }
    }

    fn is_subset_of(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Off, _) | (_, Self::On) => true,
            (Self::On, _) => false,
            (Self::Allowlist(_), Self::Off) => false,
            (Self::Allowlist(a), Self::Allowlist(b)) => a.is_subset(b),
        }
    }

    fn is_empty(&self) -> bool {
        matches!(self, Self::Off)
    }
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Off => write!(f, "off"),
            Self::On => write!(f, "on"),
            Self::Allowlist(set) => write!(f, "allowlist({})", set.len()),
        }
    }
}

/// The stream mode of one account on one connection (snapshot part 1 `mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    /// Chunks stay authoritative; stream frames are compared only.
    Shadow,
    /// The stream carries member rooms.
    Live,
    /// As live, plus seq-equal rooms are skipped at catch-up.
    Gated,
}

impl StreamMode {
    /// Wire name used in snapshot part 1.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shadow => "shadow",
            Self::Live => "live",
            Self::Gated => "gated",
        }
    }

    /// `live` or `gated`: clients rely on the stream.
    pub fn is_relied_on(self) -> bool {
        !matches!(self, Self::Shadow)
    }
}

/// Effective user-stream configuration (after precedence and intersection).
#[derive(Debug, Clone, Default)]
pub struct UserStreamConfig {
    /// Who may open the stream (default mode `shadow`).
    pub stream: Access,
    /// Effective LIVE (= LIVE ∩ STREAM; off while the stream is off).
    pub live: Access,
    /// Effective GATED (= GATED ∩ LIVE ∩ STREAM).
    pub gated: Access,
    /// `KUZZ_STREAM_EPOCH`, required while the stream is not off.
    pub epoch: String,
    /// Warnings found while resolving (logged once at startup).
    pub warnings: Vec<String>,
}

/// Raw switch values, as read from the environment.
#[derive(Debug, Clone, Default)]
pub struct UserStreamEnv {
    /// `KUZZ_USER_STREAM`.
    pub stream: Option<String>,
    /// `KUZZ_USER_STREAM_LIVE`.
    pub live: Option<String>,
    /// `KUZZ_USER_STREAM_GATED`.
    pub gated: Option<String>,
    /// `KUZZ_STREAM_EPOCH`.
    pub epoch: Option<String>,
}

impl UserStreamEnv {
    /// Read the four variables from the process environment.
    pub fn from_process_env() -> Self {
        let var = |name: &str| std::env::var(name).ok();
        Self {
            stream: var("KUZZ_USER_STREAM"),
            live: var("KUZZ_USER_STREAM_LIVE"),
            gated: var("KUZZ_USER_STREAM_GATED"),
            epoch: var("KUZZ_STREAM_EPOCH"),
        }
    }
}

/// Deployment facts the refusals depend on.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeploymentFacts {
    /// `KUZZ_MESH=on`.
    pub mesh_enabled: bool,
    /// `KUZZ_REPLICA_READ_MAX_AGE_MS` > 0 **and** a read database URL.
    pub replica_routing: bool,
}

impl UserStreamConfig {
    /// Resolve the switches. `Err` is a startup refusal.
    pub fn resolve(env: &UserStreamEnv, facts: DeploymentFacts) -> Result<Self, String> {
        let mut warnings = Vec::new();
        let stream = Access::parse("KUZZ_USER_STREAM", env.stream.as_deref(), &mut warnings)?;
        let epoch = env
            .epoch
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .to_string();

        if stream.is_empty() {
            // LIVE and GATED are ignored here; a malformed one only warns,
            // so a rollback to `off` always starts.
            let mut ignored = Vec::new();
            for (name, raw) in [
                ("KUZZ_USER_STREAM_LIVE", env.live.as_deref()),
                ("KUZZ_USER_STREAM_GATED", env.gated.as_deref()),
            ] {
                match Access::parse(name, raw, &mut warnings) {
                    Ok(Access::Off) => {}
                    Ok(access) => ignored.push(format!("{name}={access}")),
                    Err(error) => {
                        warnings.push(format!("KUZZ_USER_STREAM is off: ignoring {error}"))
                    }
                }
            }
            if !ignored.is_empty() {
                warnings.push(format!(
                    "KUZZ_USER_STREAM is off: ignoring {}",
                    ignored.join(" and ")
                ));
            }
            return Ok(Self {
                stream,
                live: Access::Off,
                gated: Access::Off,
                epoch,
                warnings,
            });
        }

        if epoch.is_empty() {
            return Err("KUZZ_USER_STREAM is not off but KUZZ_STREAM_EPOCH is unset".to_string());
        }
        if epoch.chars().count() > MAX_EPOCH_CHARS {
            return Err(format!(
                "KUZZ_STREAM_EPOCH is longer than {MAX_EPOCH_CHARS} characters"
            ));
        }
        if facts.mesh_enabled {
            return Err(
                "KUZZ_USER_STREAM is single-process: refusing with KUZZ_MESH=on".to_string(),
            );
        }
        let live_raw = Access::parse("KUZZ_USER_STREAM_LIVE", env.live.as_deref(), &mut warnings)?;
        let gated_raw = Access::parse(
            "KUZZ_USER_STREAM_GATED",
            env.gated.as_deref(),
            &mut warnings,
        )?;

        if !live_raw.is_subset_of(&stream) {
            warnings.push(format!(
                "KUZZ_USER_STREAM_LIVE={live_raw} is not inside KUZZ_USER_STREAM={stream}: \
                 accounts outside the stream set get no stream"
            ));
        }
        if !gated_raw.is_subset_of(&live_raw) || !gated_raw.is_subset_of(&stream) {
            warnings.push(format!(
                "KUZZ_USER_STREAM_GATED={gated_raw} is not inside KUZZ_USER_STREAM_LIVE={live_raw} \
                 and KUZZ_USER_STREAM={stream}: those accounts run at the highest mode every wider \
                 set allows"
            ));
        }
        let live = live_raw.intersect(&stream);
        let gated = gated_raw.intersect(&live);

        if !gated.is_empty() && facts.replica_routing {
            return Err(
                "KUZZ_USER_STREAM_GATED is effective while replica routing is on \
                 (KUZZ_REPLICA_READ_MAX_AGE_MS > 0 with READ_DATABASE_URL): snapshot and \
                 catch-up must read the same database"
                    .to_string(),
            );
        }

        Ok(Self {
            stream,
            live,
            gated,
            epoch,
            warnings,
        })
    }

    /// Whether the stream is off for everyone.
    pub fn is_off(&self) -> bool {
        self.stream.is_empty()
    }

    /// The account's mode, or `None` when it may not open the stream.
    pub fn mode_for(&self, pubkey_hex: &str) -> Option<StreamMode> {
        if !self.stream.contains(pubkey_hex) {
            return None;
        }
        Some(if self.gated.contains(pubkey_hex) {
            StreamMode::Gated
        } else if self.live.contains(pubkey_hex) {
            StreamMode::Live
        } else {
            StreamMode::Shadow
        })
    }

    /// One line with the effective accounts per mode and the epoch.
    pub fn summary(&self) -> String {
        if self.is_off() {
            return "user stream: off".to_string();
        }
        format!(
            "user stream: shadow={} live={} gated={} epoch={}",
            count_difference(&self.stream, &self.live),
            count_difference(&self.live, &self.gated),
            count_difference(&self.gated, &Access::Off),
            self.epoch
        )
    }
}

/// Accounts in `wide` but not in `narrow`, as a count or `all` / `all-but-N`.
fn count_difference(wide: &Access, narrow: &Access) -> String {
    match (wide, narrow) {
        (Access::Off, _) => "0".to_string(),
        (Access::On, Access::Off) => "all".to_string(),
        (Access::On, Access::On) => "0".to_string(),
        (Access::On, Access::Allowlist(set)) => format!("all-but-{}", set.len()),
        (Access::Allowlist(set), Access::Off) => set.len().to_string(),
        (Access::Allowlist(_), Access::On) => "0".to_string(),
        (Access::Allowlist(a), Access::Allowlist(b)) => a.difference(b).count().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char) -> String {
        std::iter::repeat_n(c, 64).collect()
    }

    fn env(stream: &str, live: &str, gated: &str, epoch: &str) -> UserStreamEnv {
        let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
        UserStreamEnv {
            stream: opt(stream),
            live: opt(live),
            gated: opt(gated),
            epoch: opt(epoch),
        }
    }

    fn resolve(e: UserStreamEnv) -> Result<UserStreamConfig, String> {
        UserStreamConfig::resolve(&e, DeploymentFacts::default())
    }

    #[test]
    fn default_env_is_off_for_everyone() {
        let config = resolve(UserStreamEnv::default()).expect("off resolves");
        assert!(config.is_off());
        assert!(config.warnings.is_empty());
        assert_eq!(config.mode_for(&key('a')), None);
        assert_eq!(config.summary(), "user stream: off");
    }

    #[test]
    fn stream_off_ignores_live_and_gated_with_a_warning() {
        let config = resolve(env("off", "on", "on", "")).expect("off never refuses");
        assert!(config.is_off());
        assert_eq!(config.live, Access::Off);
        assert_eq!(config.gated, Access::Off);
        assert_eq!(config.warnings.len(), 1);
        assert_eq!(config.mode_for(&key('a')), None);
    }

    #[test]
    fn highest_mode_needs_every_wider_set() {
        let (a, b, c) = (key('a'), key('b'), key('c'));
        let config = resolve(env(
            &format!("allowlist:{a},{b},{c}"),
            &format!("allowlist:{a},{b}"),
            &format!("allowlist:{a},{c}"),
            "e1",
        ))
        .expect("resolves");
        assert_eq!(config.mode_for(&a), Some(StreamMode::Gated));
        assert_eq!(config.mode_for(&b), Some(StreamMode::Live));
        // In GATED and STREAM but not LIVE: shadow, with one warning.
        assert_eq!(config.mode_for(&c), Some(StreamMode::Shadow));
        assert_eq!(config.mode_for(&key('d')), None);
        assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
        assert_eq!(
            config.summary(),
            "user stream: shadow=1 live=1 gated=1 epoch=e1"
        );
    }

    #[test]
    fn live_wider_than_stream_warns_and_narrows() {
        let a = key('a');
        let config = resolve(env(&format!("allowlist:{a}"), "on", "", "e1")).expect("warns only");
        assert_eq!(config.warnings.len(), 1);
        assert_eq!(config.live, Access::Allowlist([a.clone()].into()));
        assert_eq!(config.mode_for(&a), Some(StreamMode::Live));
        assert_eq!(config.mode_for(&key('b')), None);
    }

    #[test]
    fn allowlist_entries_are_normalised_and_typos_refused() {
        let upper = "A".repeat(64);
        let config = resolve(env(&format!("allowlist: {upper} "), "", "", "e1")).expect("ok");
        assert_eq!(config.mode_for(&"a".repeat(64)), Some(StreamMode::Shadow));
        for bad in [
            format!("allowlist:{}", "a".repeat(63)),
            format!("allowlist:{}", "g".repeat(64)),
            "yes".to_string(),
        ] {
            assert!(
                resolve(env(&bad, "", "", "e1")).is_err(),
                "{bad} must refuse"
            );
        }
        assert!(resolve(env("on", "bogus", "", "e1")).is_err());
        assert!(resolve(env("on", "", "allowlist:xyz", "e1")).is_err());
    }

    /// Review S4: empty entries are skipped and an empty list is `off`.
    #[test]
    fn empty_allowlist_entries_are_skipped() {
        let (a, b) = (key('a'), key('b'));
        let config = resolve(env(&format!("allowlist:{a}, ,{b},"), "", "", "e1")).expect("ok");
        assert_eq!(config.stream, Access::Allowlist([a.clone(), b].into()));
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);

        // A narrowed LIVE left as `allowlist:` is off, with a warning.
        let config = resolve(env("on", "allowlist:", "", "e1")).expect("ok");
        assert_eq!(config.live, Access::Off);
        assert_eq!(config.mode_for(&a), Some(StreamMode::Shadow));
        assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);

        let config = resolve(env("allowlist: , ", "", "", "")).expect("ok");
        assert!(config.is_off());
        assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
    }

    /// Review S4: with the stream off, a malformed LIVE or GATED only warns
    /// (`--check-config` prints the warnings).
    #[test]
    fn stream_off_never_refuses_malformed_live_or_gated() {
        let config = resolve(env(
            "off",
            "bogus",
            &format!("allowlist:{}", "a".repeat(63)),
            "",
        ))
        .expect("off never refuses");
        assert!(config.is_off());
        assert_eq!(config.live, Access::Off);
        assert_eq!(config.gated, Access::Off);
        assert_eq!(config.warnings.len(), 2, "{:?}", config.warnings);
        assert!(config.warnings.iter().all(|w| w.contains("ignoring")));

        let config = resolve(env("", "on", "bogus", "")).expect("unset stream never refuses");
        assert_eq!(config.warnings.len(), 2, "{:?}", config.warnings);
    }

    #[test]
    fn refusals() {
        assert!(resolve(env("on", "", "", "")).is_err(), "no epoch");
        assert!(resolve(env("on", "", "", "   ")).is_err(), "blank epoch");
        assert!(
            resolve(env("on", "", "", &"e".repeat(65))).is_err(),
            "long epoch"
        );
        assert!(resolve(env("on", "", "", &"e".repeat(64))).is_ok());
        let mesh = DeploymentFacts {
            mesh_enabled: true,
            replica_routing: false,
        };
        assert!(UserStreamConfig::resolve(&env("on", "", "", "e1"), mesh).is_err());
        assert!(UserStreamConfig::resolve(&env("off", "", "", ""), mesh).is_ok());
        let replica = DeploymentFacts {
            mesh_enabled: false,
            replica_routing: true,
        };
        // Effective GATED with replica routing refuses; GATED outside LIVE does not.
        assert!(UserStreamConfig::resolve(&env("on", "on", "on", "e1"), replica).is_err());
        assert!(UserStreamConfig::resolve(&env("on", "", "on", "e1"), replica).is_ok());
        assert!(UserStreamConfig::resolve(&env("on", "on", "", "e1"), replica).is_ok());
    }

    /// Every rollback row of the plan (§5.2) narrows switches; none may refuse.
    #[test]
    fn rollback_rows_always_start() {
        let a = key('a');
        let allow = format!("allowlist:{a}");
        let trailing = format!("allowlist:{a},");
        for (stream, live, gated) in [
            ("on", "allowlist:", ""),
            ("on", trailing.as_str(), "allowlist:"),
            ("off", "allowlist:", "bogus"),
            ("off", "on", "on"),
            ("off", allow.as_str(), allow.as_str()),
            (allow.as_str(), "", allow.as_str()),
            (allow.as_str(), "on", "on"),
            ("on", allow.as_str(), "on"),
            ("on", "on", allow.as_str()),
        ] {
            let config = resolve(env(stream, live, gated, "e2"));
            assert!(config.is_ok(), "{stream} / {live} / {gated}: {config:?}");
        }
    }
}
