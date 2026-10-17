//! Kuzz's own "Sign in with ChatGPT" for the ChatGPT plan provider.
//! Agents share the saved login through the same config root.

use kuzz_agent_pkg::auth::{
    chatgpt::{ChatGptLogin, ChatGptSignIn, ChatGptStatus},
    DefaultBrowserOpener,
};
use tokio::sync::oneshot;

/// Stops the running sign-in when a newer one starts.
static REPLACE: std::sync::Mutex<Option<oneshot::Sender<()>>> = std::sync::Mutex::new(None);
/// Held for a whole sign-in; a replaced one closes its listener before letting go.
static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn login() -> Result<std::sync::Arc<ChatGptLogin>, String> {
    ChatGptLogin::new(crate::build_identity::demo_config_home()?).map_err(|e| e.to_string())
}

fn user_message(error: kuzz_agent_pkg::types::AgentError) -> String {
    let text = error.to_string();
    text.split_once("ChatGPT plan: ")
        .map(|(_, rest)| rest.to_owned())
        .unwrap_or(text)
}

#[tauri::command]
pub async fn chatgpt_plan_status() -> Result<ChatGptStatus, String> {
    Ok(login()?.status())
}

/// The UI frees its button once the user is back in Kuzz, so pressing it
/// again replaces a sign-in still waiting on a closed browser tab.
#[tauri::command]
pub async fn chatgpt_plan_sign_in() -> Result<ChatGptSignIn, String> {
    let (replace, mut replaced) = oneshot::channel::<()>();
    if let Some(previous) = REPLACE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .replace(replace)
    {
        let _ = previous.send(());
    }
    let _running = RUNNING.lock().await;
    // Replaced while queued: never open a browser for it.
    if !matches!(replaced.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
        return Err("a newer sign-in replaced this one".into());
    }
    login()?
        .sign_in_until(&DefaultBrowserOpener, async move {
            let _ = replaced.await;
        })
        .await
        .map_err(user_message)
}

/// Returns whether OpenAI confirmed the session revocation.
#[tauri::command]
pub async fn chatgpt_plan_sign_out() -> Result<bool, String> {
    login()?.sign_out().await.map_err(user_message)
}
