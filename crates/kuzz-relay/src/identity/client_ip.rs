//! Client IP resolution behind trusted reverse proxies (plan §5.2 gate, §6.6).
//!
//! The `/auth/*` per-IP buckets key on the client address. Behind a reverse
//! proxy the TCP peer is the proxy, so without forwarding support every client
//! shares one bucket and `/auth/oidc/complete` collapses to a deployment-wide
//! 10/min. `AUTH_TRUSTED_PROXY_CIDRS` lists the proxies whose forwarding
//! headers are believed:
//!
//! - The peer must itself be trusted; otherwise its headers are ignored and the
//!   peer address is the client (a direct client cannot spoof its bucket).
//! - `X-Forwarded-For` is walked right to left, skipping trusted hops; the
//!   first untrusted address is the client. Without XFF, the RFC 7239
//!   `Forwarded: for=` chain is walked the same way.
//! - A malformed hop stops the walk: the request is keyed on the nearest
//!   trusted proxy instead (never on attacker-chosen text).
//! - The literal entry `unix` trusts Unix-socket peers (no peer IP); without it
//!   a UDS request has no client IP and uses the deployment-wide bucket.

use std::net::IpAddr;

use axum::http::HeaderMap;

/// One CIDR block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cidr {
    network: IpAddr,
    prefix: u8,
}

impl Cidr {
    fn parse(raw: &str) -> Result<Self, String> {
        let (addr, prefix) = match raw.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (raw, None),
        };
        let network: IpAddr = addr
            .trim()
            .parse()
            .map_err(|_| format!("invalid proxy address {raw:?}"))?;
        let max = if network.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            None => max,
            Some(p) => p
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= max)
                .ok_or_else(|| format!("invalid prefix length in {raw:?}"))?,
        };
        Ok(Self { network, prefix })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, canonical(ip)) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// IPv4-mapped IPv6 (`::ffff:a.b.c.d`) compares as IPv4.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// The configured trusted reverse proxies (`AUTH_TRUSTED_PROXY_CIDRS`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedProxies {
    nets: Vec<Cidr>,
    trust_unix: bool,
}

impl TrustedProxies {
    /// Parse a comma-separated list of CIDRs / addresses and the literal `unix`.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let mut proxies = Self::default();
        for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            if entry.eq_ignore_ascii_case("unix") {
                proxies.trust_unix = true;
            } else {
                proxies.nets.push(Cidr::parse(entry)?);
            }
        }
        Ok(proxies)
    }

    /// Whether nothing is trusted (forwarding headers are never read).
    pub fn is_empty(&self) -> bool {
        self.nets.is_empty() && !self.trust_unix
    }

    fn trusts(&self, ip: IpAddr) -> bool {
        self.nets.iter().any(|net| net.contains(ip))
    }

    /// The client address for a request from `peer` (`None` = Unix socket).
    pub fn client_ip(&self, peer: Option<IpAddr>, headers: &HeaderMap) -> Option<IpAddr> {
        let peer_trusted = match peer {
            Some(ip) => self.trusts(ip),
            None => self.trust_unix,
        };
        if !peer_trusted {
            return peer;
        }
        let hops = forwarded_hops(headers);
        let mut nearest_trusted = peer;
        for hop in hops.iter().rev() {
            match hop {
                Some(ip) if self.trusts(*ip) => nearest_trusted = Some(*ip),
                Some(ip) => return Some(canonical(*ip)),
                None => return nearest_trusted,
            }
        }
        nearest_trusted
    }
}

/// The forwarding chain, client first: `X-Forwarded-For` (all header lines),
/// else `Forwarded` `for=` values. `None` marks an unparsable hop.
fn forwarded_hops(headers: &HeaderMap) -> Vec<Option<IpAddr>> {
    let xff: Vec<Option<IpAddr>> = headers
        .get_all("x-forwarded-for")
        .iter()
        .flat_map(|value| match value.to_str() {
            Ok(value) => value.split(',').map(parse_hop).collect::<Vec<_>>(),
            Err(_) => vec![None],
        })
        .collect();
    if !xff.is_empty() {
        return xff;
    }
    headers
        .get_all(axum::http::header::FORWARDED)
        .iter()
        .flat_map(|value| match value.to_str() {
            Ok(value) => value
                .split(',')
                .filter_map(|element| {
                    element.split(';').find_map(|pair| {
                        let (key, value) = pair.split_once('=')?;
                        key.trim()
                            .eq_ignore_ascii_case("for")
                            .then(|| parse_hop(value))
                    })
                })
                .collect::<Vec<_>>(),
            Err(_) => vec![None],
        })
        .collect()
}

/// Parse one hop: `1.2.3.4`, `1.2.3.4:567`, `"[2001:db8::1]:80"`, `2001:db8::1`.
fn parse_hop(raw: &str) -> Option<IpAddr> {
    let hop = raw.trim().trim_matches('"');
    if let Ok(ip) = hop.parse::<IpAddr>() {
        return Some(ip);
    }
    if let Some(rest) = hop.strip_prefix('[') {
        return rest.split_once(']')?.0.parse().ok();
    }
    let (host, port) = hop.rsplit_once(':')?;
    port.parse::<u16>().ok()?;
    host.parse::<std::net::Ipv4Addr>().ok().map(IpAddr::V4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, value.parse().unwrap());
        }
        map
    }

    #[test]
    fn parses_cidrs_addresses_and_unix() {
        let proxies = TrustedProxies::parse("10.0.0.0/8, 192.168.1.5,fd00::/8,unix").unwrap();
        assert!(proxies.trusts(ip("10.200.3.4")));
        assert!(proxies.trusts(ip("192.168.1.5")));
        assert!(!proxies.trusts(ip("192.168.1.6")));
        assert!(proxies.trusts(ip("fd12::1")));
        assert!(
            proxies.trusts(ip("::ffff:10.1.1.1")),
            "v4-mapped matches v4"
        );
        assert!(proxies.trust_unix);
        assert!(TrustedProxies::parse("").unwrap().is_empty());
        assert!(TrustedProxies::parse("10.0.0.0/33").is_err());
        assert!(TrustedProxies::parse("nope").is_err());
        assert!(TrustedProxies::parse("0.0.0.0/0")
            .unwrap()
            .trusts(ip("8.8.8.8")));
    }

    #[test]
    fn untrusted_peer_headers_are_ignored() {
        let proxies = TrustedProxies::parse("10.0.0.0/8").unwrap();
        let h = headers(&[("x-forwarded-for", "1.1.1.1")]);
        assert_eq!(
            proxies.client_ip(Some(ip("8.8.8.8")), &h),
            Some(ip("8.8.8.8"))
        );
        // Nothing configured: the peer, always.
        let none = TrustedProxies::default();
        assert_eq!(
            none.client_ip(Some(ip("10.0.0.1")), &h),
            Some(ip("10.0.0.1"))
        );
        assert_eq!(none.client_ip(None, &h), None);
    }

    #[test]
    fn xff_is_walked_right_to_left_past_trusted_hops() {
        let proxies = TrustedProxies::parse("10.0.0.0/8").unwrap();
        // Client spoofs a leftmost entry; the proxy appends the real client.
        let h = headers(&[("x-forwarded-for", "6.6.6.6, 203.0.113.7, 10.0.0.9")]);
        assert_eq!(
            proxies.client_ip(Some(ip("10.0.0.1")), &h),
            Some(ip("203.0.113.7"))
        );
        // Multiple header lines form one list.
        let h = headers(&[
            ("x-forwarded-for", "198.51.100.1"),
            ("x-forwarded-for", "10.0.0.9"),
        ]);
        assert_eq!(
            proxies.client_ip(Some(ip("10.0.0.1")), &h),
            Some(ip("198.51.100.1"))
        );
        // No header: the proxy itself.
        assert_eq!(
            proxies.client_ip(Some(ip("10.0.0.1")), &HeaderMap::new()),
            Some(ip("10.0.0.1"))
        );
    }

    #[test]
    fn malformed_hop_stops_at_nearest_trusted_proxy() {
        let proxies = TrustedProxies::parse("10.0.0.0/8").unwrap();
        let h = headers(&[("x-forwarded-for", "1.2.3.4, garbage, 10.0.0.9")]);
        assert_eq!(
            proxies.client_ip(Some(ip("10.0.0.1")), &h),
            Some(ip("10.0.0.9"))
        );
    }

    #[test]
    fn forwarded_header_is_used_without_xff() {
        let proxies = TrustedProxies::parse("10.0.0.0/8").unwrap();
        let h = headers(&[(
            "forwarded",
            r#"for=192.0.2.60;proto=https, for="[2001:db8::7]:4711";by=10.0.0.2"#,
        )]);
        assert_eq!(
            proxies.client_ip(Some(ip("10.0.0.1")), &h),
            Some(ip("2001:db8::7"))
        );
        let h = headers(&[("forwarded", "for=192.0.2.60:8080")]);
        assert_eq!(
            proxies.client_ip(Some(ip("10.0.0.1")), &h),
            Some(ip("192.0.2.60"))
        );
    }

    #[test]
    fn unix_socket_peer_needs_explicit_trust() {
        let h = headers(&[("x-forwarded-for", "203.0.113.7")]);
        let without = TrustedProxies::parse("10.0.0.0/8").unwrap();
        assert_eq!(without.client_ip(None, &h), None);
        let with = TrustedProxies::parse("unix").unwrap();
        assert_eq!(with.client_ip(None, &h), Some(ip("203.0.113.7")));
        assert_eq!(with.client_ip(None, &HeaderMap::new()), None);
    }
}
