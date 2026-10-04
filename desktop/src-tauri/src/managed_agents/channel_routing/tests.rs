use super::*;

fn record(starred: bool, local: bool) -> ManagedAgentRecord {
    let mut record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
        "pubkey": "ab".repeat(32),
        "name": "Honey",
        "private_key_nsec": "",
        "relay_url": "",
        "acp_command": "buzz-acp",
        "agent_command": "goose",
        "agent_args": [],
        "mcp_command": "",
        "turn_timeout_seconds": 300,
        "created_at": "",
        "updated_at": "",
    }))
    .expect("minimal record deserializes");
    record.is_default_ai = starred;
    if !local {
        record.backend = BackendKind::Provider {
            id: "cloud".into(),
            config: serde_json::Value::Null,
        };
    }
    record
}

/// The full table: mode × star × backend × owner known. Off and Smart
/// routing must leave the starred record ordinary — that is where today's
/// behavior changes, so deleting either arm fails here.
#[test]
fn routing_role_for_covers_every_mode_star_backend_and_owner() {
    use ChannelRoutingMode::*;
    use RoutingRole as R;
    let cases = [
        // (mode, starred, local, owner known) -> role
        (Off, true, true, true, R::None),
        (Off, true, false, true, R::None),
        (Off, true, true, false, R::None),
        (Host, true, true, true, R::Dispatcher),
        (Host, true, false, true, R::Dispatcher),
        (Host, true, true, false, R::Dispatcher),
        (Host, true, false, false, R::Dispatcher),
        (Lead, true, true, true, R::Lead),
        (Lead, true, false, true, R::None),
        (Lead, true, true, false, R::None),
        (Lead, true, false, false, R::None),
        (DesktopRouter, true, true, true, R::None),
        (DesktopRouter, true, false, true, R::None),
        (DesktopRouter, true, true, false, R::None),
    ];
    for (mode, starred, local, owner_known, expected) in cases {
        let owner = owner_known.then_some("owner-hex");
        assert_eq!(
            routing_role_for(&record(starred, local), mode, owner),
            expected,
            "{mode:?} starred={starred} local={local} owner={owner_known}"
        );
    }
    // An unstarred record never has a role, in any combination.
    for mode in [Off, Host, Lead, DesktopRouter] {
        for local in [true, false] {
            for owner in [Some("owner-hex"), None] {
                assert_eq!(
                    routing_role_for(&record(false, local), mode, owner),
                    R::None
                );
            }
        }
    }
    // A blank owner is no owner.
    assert_eq!(
        routing_role_for(&record(true, true), Lead, Some("  ")),
        R::None
    );
}

#[test]
fn a_missing_or_damaged_file_reads_as_host() {
    assert_eq!(parse_channel_routing(None), ChannelRoutingMode::Host);
    assert_eq!(
        parse_channel_routing(Some("not json")),
        ChannelRoutingMode::Host
    );
    assert_eq!(
        parse_channel_routing(Some(r#"{"mode":"sideways"}"#)),
        ChannelRoutingMode::Host
    );
    assert_eq!(
        parse_channel_routing(Some(r#"{"mode":"off"}"#)),
        ChannelRoutingMode::Off
    );
    assert_eq!(
        parse_channel_routing(Some(r#"{"mode":"desktop-router"}"#)),
        ChannelRoutingMode::DesktopRouter
    );
}

#[test]
fn save_then_load_round_trips_every_mode() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(CHANNEL_ROUTING_FILE);
    assert_eq!(
        load_channel_routing_at(&path).expect("missing file loads"),
        ChannelRoutingMode::Host
    );
    for mode in [
        ChannelRoutingMode::Off,
        ChannelRoutingMode::Host,
        ChannelRoutingMode::Lead,
        ChannelRoutingMode::DesktopRouter,
    ] {
        save_channel_routing_at(&path, mode).expect("save");
        assert_eq!(load_channel_routing_at(&path).expect("load"), mode);
    }
    let stored = std::fs::read_to_string(&path).expect("read");
    assert!(
        stored.contains(r#""mode": "desktop-router""#),
        "got: {stored}"
    );
}

#[test]
fn smart_routing_is_the_only_unselectable_mode_in_this_build() {
    assert!(ChannelRoutingMode::Off.is_selectable());
    assert!(ChannelRoutingMode::Host.is_selectable());
    assert!(ChannelRoutingMode::Lead.is_selectable());
    assert!(!ChannelRoutingMode::DesktopRouter.is_selectable());
}
