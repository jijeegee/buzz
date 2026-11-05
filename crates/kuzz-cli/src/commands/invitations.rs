//! Group invitations sent to this identity.
//!
//! Adding someone who is not yet allowed to join directly leaves a pending
//! invitation on the relay. The relay does not push it, so `kuzz invitations
//! list` is how an agent finds it.

use crate::client::KuzzClient;
use crate::error::{classify_relay, CliError};
use crate::validate::validate_uuid;
use crate::InvitationsCmd;

fn response_path(id: &str, action: &str) -> Result<String, CliError> {
    validate_uuid(id)?;
    Ok(format!("/api/group-invitations/{id}/{action}"))
}

async fn cmd_list(client: &KuzzClient) -> Result<(), CliError> {
    let resp = client.get_authed("/api/group-invitations").await?;
    println!("{resp}");
    Ok(())
}

async fn cmd_respond(client: &KuzzClient, id: &str, action: &str) -> Result<(), CliError> {
    let path = response_path(id, action)?;
    let resp = client
        .post_json_once_authed(&path, &serde_json::json!({}))
        .await
        .map_err(classify_relay)?;
    println!("{resp}");
    Ok(())
}

pub async fn dispatch(cmd: InvitationsCmd, client: &KuzzClient) -> Result<(), CliError> {
    match cmd {
        InvitationsCmd::List => cmd_list(client).await,
        InvitationsCmd::Accept { invitation } => cmd_respond(client, &invitation, "accept").await,
        InvitationsCmd::Decline { invitation } => cmd_respond(client, &invitation, "decline").await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "41a46ce7-38d8-4771-8047-cadeb6f610ce";

    #[test]
    fn response_path_names_the_invitation_and_action() {
        assert_eq!(
            response_path(ID, "accept").unwrap(),
            format!("/api/group-invitations/{ID}/accept")
        );
    }

    #[test]
    fn response_path_refuses_anything_but_a_uuid() {
        assert!(response_path("../../auth/me", "accept").is_err());
    }
}
