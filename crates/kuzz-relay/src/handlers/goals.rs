//! Goal edits (kind 40111): rows and the stored event change in one
//! transaction, then the event goes out so open apps reload.
use super::ingest::{IngestError, IngestResult};
use crate::state::{AppState, ThreadedChannelVisibility};
use kuzz_core::kind::KIND_GOAL_OPS;
use kuzz_core::TenantContext;
use kuzz_db::goals::{parse_goal_ops, GoalOutcome, GoalWrite};
use kuzz_db::DbError;
use nostr::Event;
use std::sync::Arc;
use uuid::Uuid;

/// Apply an event whose channel already passed ingest's write gates. The
/// outcome goes back as JSON in the result message: deleted goals can no
/// longer be read, so this is the writer's only record of them.
pub(crate) async fn accept(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    event: &Event,
    channel: Uuid,
    visibility: Option<ThreadedChannelVisibility>,
) -> Result<IngestResult, IngestError> {
    let ops = parse_goal_ops(&event.content).map_err(goal_error)?;
    let write = state
        .db
        .apply_goal_ops(tenant.community(), channel, event, &ops)
        .await
        .map_err(goal_error)?;
    let outcome = match write {
        GoalWrite::Applied(stored, outcome) => {
            super::event::dispatch_persistent_event(
                tenant,
                state,
                &stored,
                KIND_GOAL_OPS,
                &event.pubkey.to_hex(),
                visibility,
            )
            .await;
            Some(outcome)
        }
        GoalWrite::NoEffect(outcome) => Some(outcome),
        GoalWrite::Duplicate => None,
    };
    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message: outcome_message(outcome)?,
    })
}

fn outcome_message(outcome: Option<GoalOutcome>) -> Result<String, IngestError> {
    let Some(outcome) = outcome else {
        return Ok("duplicate:".to_owned());
    };
    serde_json::to_string(&outcome).map_err(|e| IngestError::Internal(format!("goal outcome: {e}")))
}

fn goal_error(error: DbError) -> IngestError {
    match error {
        DbError::GoalRejected(message) => IngestError::Rejected(format!("invalid: {message}")),
        other => IngestError::Internal(format!("goal storage: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duplicate_has_no_outcome_and_a_write_reports_json() {
        assert_eq!(outcome_message(None).unwrap(), "duplicate:");
        let message = outcome_message(Some(GoalOutcome {
            parent: Some("p".into()),
            deleted: vec!["a".into()],
            missing: Vec::new(),
        }))
        .unwrap();
        let parsed: GoalOutcome = serde_json::from_str(&message).unwrap();
        assert_eq!(parsed.deleted, ["a"]);
    }

    #[test]
    fn a_broken_rule_is_the_writers_fault_and_storage_is_ours() {
        assert!(matches!(
            goal_error(DbError::GoalRejected("no".into())),
            IngestError::Rejected(m) if m == "invalid: no"
        ));
        assert!(matches!(
            goal_error(DbError::InvalidData("x".into())),
            IngestError::Internal(_)
        ));
    }
}
