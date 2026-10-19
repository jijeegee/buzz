//! Per-client-IP WebSocket admission: a cap on open connections and on new
//! connections per minute, checked before the upgrade and before the
//! host→community lookup so a flood costs neither a socket nor a DB query.
//!
//! The client IP is the peer, or — behind a trusted reverse proxy
//! (`AUTH_TRUSTED_PROXY_CIDRS`) — the client its forwarding headers name. When
//! it cannot be resolved the request is admitted: one shared bucket for every
//! unknown client would let a single client lock everyone out.
//!
//! Defaults are generous on purpose. One household or carrier-grade NAT can sit
//! behind a single address, and one person's desktop opens a connection per
//! account and per running agent.

use std::net::IpAddr;
use std::sync::Arc;

use dashmap::DashMap;

/// Default cap on concurrently open WebSocket connections from one IP.
pub const DEFAULT_MAX_OPEN_PER_IP: u32 = 200;
/// Default cap on new WebSocket connections from one IP per minute.
pub const DEFAULT_NEW_PER_IP_PER_MIN: u64 = 600;

/// Process-local open-connection counts keyed by client IP.
pub type OpenConnectionsByIp = Arc<DashMap<IpAddr, u32>>;

/// Holds one open-connection slot for `ip` until dropped.
#[derive(Debug)]
pub struct IpConnectionSlot {
    open: OpenConnectionsByIp,
    ip: IpAddr,
}

impl Drop for IpConnectionSlot {
    fn drop(&mut self) {
        // Decrement and remove under the same shard lock so a concurrent
        // acquire never observes a stale zero entry.
        self.open.remove_if_mut(&self.ip, |_, count| {
            *count = count.saturating_sub(1);
            *count == 0
        });
    }
}

/// Take an open-connection slot for `ip`, or `None` when it already holds
/// `max_open` connections. `max_open == 0` disables the cap.
pub fn try_acquire_slot(
    open: &OpenConnectionsByIp,
    ip: IpAddr,
    max_open: u32,
) -> Option<IpConnectionSlot> {
    let mut entry = open.entry(ip).or_insert(0);
    if max_open != 0 && *entry >= max_open {
        return None;
    }
    *entry += 1;
    drop(entry);
    Some(IpConnectionSlot {
        open: Arc::clone(open),
        ip,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([203, 0, 113, last])
    }

    #[test]
    fn caps_open_connections_per_ip_and_frees_slots_on_drop() {
        let open = OpenConnectionsByIp::default();
        let first = try_acquire_slot(&open, ip(1), 2).expect("first slot");
        let second = try_acquire_slot(&open, ip(1), 2).expect("second slot");
        assert!(
            try_acquire_slot(&open, ip(1), 2).is_none(),
            "third is over the cap"
        );
        assert!(
            try_acquire_slot(&open, ip(2), 2).is_some(),
            "another IP has its own count"
        );

        drop(first);
        let third = try_acquire_slot(&open, ip(1), 2).expect("a closed slot is reusable");
        drop(second);
        drop(third);
        assert!(
            open.get(&ip(1)).is_none(),
            "no entry is left once all close"
        );
    }

    #[test]
    fn zero_disables_the_cap() {
        let open = OpenConnectionsByIp::default();
        let slots: Vec<_> = (0..1000)
            .map(|_| try_acquire_slot(&open, ip(3), 0).expect("uncapped"))
            .collect();
        assert_eq!(*open.get(&ip(3)).unwrap(), 1000);
        drop(slots);
        assert!(open.get(&ip(3)).is_none());
    }
}
