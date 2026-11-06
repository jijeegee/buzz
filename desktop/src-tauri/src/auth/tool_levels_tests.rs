use super::*;
use crate::auth::agent_hosting::Hosting;

fn error(status: Option<u16>, code: Option<&str>) -> ApiError {
    ApiError {
        status,
        code: code.map(str::to_owned),
        message: "refused".into(),
    }
}

fn row(level: Option<&str>, class: Option<&str>, here: bool) -> api::AgentToolLevel {
    api::AgentToolLevel {
        pubkey: "ab".into(),
        level: level.map(str::to_owned),
        respond_class: class.map(str::to_owned),
        here,
    }
}

#[test]
fn defaults_are_on_only_for_owner_only() {
    assert_eq!(default_level(RespondTo::OwnerOnly), ToolLevel::Full);
    assert_eq!(default_level(RespondTo::Allowlist), ToolLevel::Limited);
    assert_eq!(default_level(RespondTo::Anyone), ToolLevel::Limited);
}

#[test]
fn a_save_keeps_the_level_and_must_succeed_only_when_on() {
    use RespondTo::*;
    use ToolLevel::*;
    // On and kept: the save fails if the call fails.
    assert_eq!(
        save_write(Some(Some(Full)), None, OwnerOnly),
        Some(SaveWrite {
            level: Full,
            must_succeed: true,
            owner_change: false
        })
    );
    assert_eq!(
        save_write(Some(Some(Full)), None, Allowlist),
        Some(SaveWrite {
            level: Full,
            must_succeed: true,
            owner_change: false
        })
    );
    // Switching to anyone sends off, and is not blocked by the call.
    assert_eq!(
        save_write(Some(Some(Full)), None, Anyone),
        Some(SaveWrite {
            level: Limited,
            must_succeed: false,
            owner_change: false
        })
    );
    // Off: sent, but a failure does not stop the save.
    assert_eq!(
        save_write(Some(Some(Limited)), None, Allowlist),
        Some(SaveWrite {
            level: Limited,
            must_succeed: false,
            owner_change: false
        })
    );
    // Unknown or never set: no call (the default write covers a null).
    assert_eq!(save_write(None, None, OwnerOnly), None);
    assert_eq!(save_write(Some(None), None, OwnerOnly), None);
    assert_eq!(save_write(None, None, Allowlist), None);
}

#[test]
fn anyone_sends_off_even_when_the_level_could_not_be_read() {
    let off = Some(SaveWrite {
        level: ToolLevel::Limited,
        must_succeed: false,
        owner_change: false,
    });
    assert_eq!(save_write(None, None, RespondTo::Anyone), off);
    assert_eq!(save_write(Some(None), None, RespondTo::Anyone), off);
}

#[test]
fn a_stale_class_stops_the_save_only_when_the_owner_changed_the_level() {
    let stale = || Err(error(Some(409), Some("respond_class_stale")));
    let kept_on = SaveWrite {
        level: ToolLevel::Full,
        must_succeed: true,
        owner_change: false,
    };
    let changed = SaveWrite {
        owner_change: true,
        ..kept_on
    };
    // Editing on a computer that doesn't run the agent: the save goes on.
    assert!(settle_write(kept_on, stale()).is_ok());
    // Any other failure with the level on still stops it.
    assert!(settle_write(kept_on, Err(error(None, None))).is_err());
    // The owner's own change must reach the relay.
    let refused = settle_write(changed, stale()).unwrap_err();
    assert!(refused.contains("computer that runs"), "{refused}");
}

#[test]
fn an_owner_change_is_always_sent_and_must_succeed() {
    use RespondTo::*;
    use ToolLevel::*;
    assert_eq!(
        save_write(None, Some(Full), Allowlist),
        Some(SaveWrite {
            level: Full,
            must_succeed: true,
            owner_change: true
        })
    );
    assert_eq!(
        save_write(Some(Some(Full)), Some(Limited), OwnerOnly),
        Some(SaveWrite {
            level: Limited,
            must_succeed: true,
            owner_change: true
        })
    );
    // Never on for anyone, even when asked.
    assert_eq!(
        save_write(None, Some(Full), Anyone),
        Some(SaveWrite {
            level: Limited,
            must_succeed: true,
            owner_change: true
        })
    );
}

#[test]
fn a_failed_call_stops_the_save_only_when_it_must_succeed() {
    let on = SaveWrite {
        level: ToolLevel::Full,
        must_succeed: true,
        owner_change: false,
    };
    let off = SaveWrite {
        level: ToolLevel::Limited,
        must_succeed: false,
        owner_change: false,
    };
    assert!(settle_write(on, Ok("set".into())).is_ok());
    assert!(settle_write(on, Err(error(None, None))).is_err());
    assert!(settle_write(off, Err(error(None, None))).is_ok());
    let owner_change = SaveWrite {
        owner_change: true,
        ..on
    };
    let stale = settle_write(
        owner_change,
        Err(error(Some(409), Some("respond_class_stale"))),
    )
    .unwrap_err();
    assert!(stale.contains("computer that runs"), "{stale}");
}

#[tokio::test]
async fn writers_call_the_route_before_saving() {
    use std::sync::Mutex;
    let order = Mutex::new(Vec::new());
    let on = SaveWrite {
        level: ToolLevel::Full,
        must_succeed: true,
        owner_change: false,
    };

    // The call runs first, then the save.
    route_then_save(
        Some(on),
        async {
            order.lock().unwrap().push("route");
            Ok("set".to_string())
        },
        || async {
            order.lock().unwrap().push("save");
            Ok::<(), String>(())
        },
    )
    .await
    .unwrap();
    assert_eq!(*order.lock().unwrap(), vec!["route", "save"]);

    // Level on and the call fails: nothing is saved.
    order.lock().unwrap().clear();
    let failed = route_then_save(
        Some(on),
        async {
            order.lock().unwrap().push("route");
            Err(error(None, None))
        },
        || async {
            order.lock().unwrap().push("save");
            Ok::<(), String>(())
        },
    )
    .await;
    assert!(failed.is_err());
    assert_eq!(*order.lock().unwrap(), vec!["route"]);

    // No call needed: the save goes on alone.
    order.lock().unwrap().clear();
    route_then_save(
        None,
        async {
            order.lock().unwrap().push("route");
            Ok(String::new())
        },
        || async {
            order.lock().unwrap().push("save");
            Ok::<(), String>(())
        },
    )
    .await
    .unwrap();
    assert_eq!(*order.lock().unwrap(), vec!["save"]);
}

#[test]
fn the_sweep_defaults_only_never_set_agents_hosted_here() {
    use RespondTo::*;
    assert_eq!(sweep_step(None, OwnerOnly), SweepStep::Skip);
    assert_eq!(
        sweep_step(Some(&row(None, None, false)), OwnerOnly),
        SweepStep::Skip
    );
    assert_eq!(
        sweep_step(Some(&row(None, None, true)), OwnerOnly),
        SweepStep::Default
    );
    // Set and in step: left alone, never re-defaulted.
    assert_eq!(
        sweep_step(
            Some(&row(Some("limited"), Some("owner-only"), true)),
            OwnerOnly
        ),
        SweepStep::Skip
    );
    // Set with another class: this host's class, level kept or lowered.
    assert_eq!(
        sweep_step(
            Some(&row(Some("full"), Some("owner-only"), true)),
            Allowlist
        ),
        SweepStep::Reconcile(ToolLevel::Full)
    );
    assert_eq!(
        sweep_step(Some(&row(Some("full"), Some("owner-only"), true)), Anyone),
        SweepStep::Reconcile(ToolLevel::Limited)
    );
    assert_eq!(
        sweep_step(Some(&row(Some("limited"), None, true)), OwnerOnly),
        SweepStep::Reconcile(ToolLevel::Limited)
    );
}

#[test]
fn revoke_is_skipped_on_dev_demo_bots_and_other_hosts() {
    assert_eq!(
        revoke_step(false, false, false, true, None),
        RevokeStep::Revoke
    );
    assert_eq!(
        revoke_step(false, false, false, true, Some(Hosting::Here)),
        RevokeStep::Revoke
    );
    assert_eq!(
        revoke_step(true, false, false, true, None),
        RevokeStep::Skip
    );
    assert_eq!(
        revoke_step(false, true, false, true, None),
        RevokeStep::Skip
    );
    assert_eq!(
        revoke_step(false, false, true, true, None),
        RevokeStep::Skip
    );
    assert_eq!(
        revoke_step(false, false, false, false, None),
        RevokeStep::Skip
    );
    assert_eq!(
        revoke_step(false, false, false, true, Some(Hosting::OtherDevice)),
        RevokeStep::Skip
    );
}

#[test]
fn a_failed_revoke_keeps_the_agent() {
    assert!(revoke_outcome(Ok(())).is_ok());
    // Already gone, or hosted elsewhere: delete here only.
    assert!(revoke_outcome(Err(error(Some(404), Some("agent_not_found")))).is_ok());
    assert!(revoke_outcome(Err(error(Some(409), Some("not_host")))).is_ok());
    // Relay unreachable: deleted here as before S1 (logged).
    assert!(revoke_outcome(Err(error(None, None))).is_ok());
    assert!(revoke_outcome(Err(error(Some(503), None))).is_ok());
    // A refusal or server error: the agent stays to retry.
    assert!(revoke_outcome(Err(error(Some(500), None))).is_err());
    assert!(revoke_outcome(Err(error(Some(403), Some("device_proof_required")))).is_err());
}
