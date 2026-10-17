//! Kuzz's own "Sign in with ChatGPT" for the ChatGPT plan provider.
//! Agents share the saved login through the same config root.

use kuzz_agent_pkg::auth::{
    chatgpt::{ChatGptLogin, ChatGptSignIn, ChatGptStatus},
    DefaultBrowserOpener,
};

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

#[tauri::command]
pub async fn chatgpt_plan_sign_in() -> Result<ChatGptSignIn, String> {
    login()?
        .sign_in(&DefaultBrowserOpener)
        .await
        .map_err(user_message)
}

/// Returns whether OpenAI confirmed the session revocation.
#[tauri::command]
pub async fn chatgpt_plan_sign_out() -> Result<bool, String> {
    login()?.sign_out().await.map_err(user_message)
}
