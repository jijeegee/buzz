//! Rule 3/6: a user-initiated Start is the manual reset for an agent parked
//! after repeated auth failures. Bound to the production start path.

use super::start_local_agent_with_preflight;
use crate::app_state::AppState;
use crate::managed_agents::admission_test_support::app_with_keyless_agent;
use tauri::Manager;

#[tokio::test]
async fn manual_start_unparks_an_auth_failed_agent() {
    let test = app_with_keyless_agent();
    let (app, state) = (test.app.handle(), test.app.state::<AppState>());
    state
        .token_auth
        .bots
        .park(&test.pubkey, "token rejected repeatedly".into());
    assert!(state.token_auth.bots.auth_failed(&test.pubkey).is_some());
    // The keyless fixture stops at the spawn's key refusal; the reset happens
    // before that, at the start of the user-initiated path.
    let _ =
        start_local_agent_with_preflight(app, &state, &test.pubkey, false, None, None, None).await;
    assert_eq!(state.token_auth.bots.auth_failed(&test.pubkey), None);
}
