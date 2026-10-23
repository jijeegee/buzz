//! `kuzz-admin regions`: the deployment's region list (global regions S6-1).
//!
//! Runs against the primary region's database (`DATABASE_URL`). The list is
//! copied from there to every other region; a new region's relay waits until
//! it sees its own row with its own link key. Steps: `docs/regions-runbook.md`.

use anyhow::{bail, Result};
use clap::Subcommand;
use kuzz_db::region::RegionAdded;
use nostr::PublicKey;

/// Region list commands.
#[derive(Subcommand)]
pub enum RegionsCommand {
    /// List regions with their address, link key, rooms and accounts.
    List,
    /// Add a region, or change its address or link key.
    ///
    /// The new region's relay logs its id and link key while it waits.
    Add {
        /// Region id: a lowercase letter, then up to 15 lowercase letters,
        /// digits or '-' (e.g. `jp`).
        #[arg(long)]
        id: String,
        /// Address apps in that region connect to (`wss://...`).
        #[arg(long)]
        connect_url: String,
        /// The region's link public key (64-char hex or npub).
        #[arg(long)]
        link_pubkey: String,
    },
    /// Remove a region that no room lives in. Accounts homed there are homed
    /// in the primary instead.
    Remove {
        /// Region id.
        #[arg(long)]
        id: String,
    },
}

/// Run a region command; the process exit code.
pub async fn run(command: RegionsCommand) -> Result<i32> {
    let db = crate::connect_db().await?;
    match command {
        RegionsCommand::List => {
            let regions = db.region_status().await?;
            if regions.is_empty() {
                println!("(no regions)");
                return Ok(0);
            }
            println!(
                "{:<16} {:<8} {:<8} {:<8} {:<16} connect_url",
                "id", "primary", "rooms", "people", "link_pubkey"
            );
            for region in regions {
                let key = region
                    .link_pubkey
                    .as_deref()
                    .map(|key| format!("{}…", &key[..12.min(key.len())]))
                    .unwrap_or_else(|| "-".into());
                println!(
                    "{:<16} {:<8} {:<8} {:<8} {:<16} {}",
                    region.id,
                    if region.is_primary { "yes" } else { "" },
                    region.rooms,
                    region.people,
                    key,
                    region.connect_url
                );
            }
            Ok(0)
        }
        RegionsCommand::Add {
            id,
            connect_url,
            link_pubkey,
        } => {
            let (id, connect_url, link_pubkey) = checked(&id, &connect_url, &link_pubkey)?;
            let added = db.add_region(&id, &connect_url, &link_pubkey).await?;
            match added {
                RegionAdded::Added => println!("added region {id} ({connect_url})"),
                RegionAdded::Updated => println!("updated region {id} ({connect_url})"),
                RegionAdded::Unchanged => println!("region {id} is already recorded as given"),
            }
            println!(
                "The row reaches every region through the copy; {id}'s relay starts once it sees it."
            );
            Ok(0)
        }
        RegionsCommand::Remove { id } => {
            let removed = db.remove_region(id.trim()).await?;
            println!(
                "removed region {}; {} accounts are now homed in the primary",
                id.trim(),
                removed.people_moved
            );
            Ok(0)
        }
    }
}

/// Check and normalise `regions add` arguments: the id, a `ws`/`wss`
/// address, and the link key as lowercase hex.
fn checked(id: &str, connect_url: &str, link_pubkey: &str) -> Result<(String, String, String)> {
    let id = id.trim();
    let mut chars = id.chars();
    let valid_id = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && id.len() <= 16
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid_id {
        bail!("region id must be a lowercase letter, then up to 15 lowercase letters, digits or '-' (got {id:?})");
    }
    let connect_url = connect_url.trim();
    let parsed = url::Url::parse(connect_url)
        .map_err(|e| anyhow::anyhow!("connect URL {connect_url:?} is not a URL: {e}"))?;
    if !matches!(parsed.scheme(), "ws" | "wss") || parsed.host_str().is_none() {
        bail!("connect URL must be ws:// or wss:// with a host (got {connect_url:?})");
    }
    let key = PublicKey::parse(link_pubkey.trim())
        .map_err(|e| anyhow::anyhow!("link key {link_pubkey:?} is not a public key: {e}"))?;
    Ok((id.to_owned(), connect_url.to_owned(), key.to_hex()))
}

#[cfg(test)]
mod tests {
    use super::checked;
    use clap::Parser;
    use nostr::ToBech32;

    #[test]
    fn add_arguments_are_checked_and_normalised() {
        let hex = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
        let npub = nostr::PublicKey::from_hex(hex)
            .unwrap()
            .to_bech32()
            .unwrap();
        assert_eq!(
            checked(" jp ", "wss://jp.example", &npub).unwrap(),
            ("jp".into(), "wss://jp.example".into(), hex.into())
        );
        assert!(checked("JP", "wss://jp.example", hex).is_err());
        assert!(checked("1jp", "wss://jp.example", hex).is_err());
        assert!(checked("jp", "https://jp.example", hex).is_err());
        assert!(checked("jp", "jp.example", hex).is_err());
        assert!(checked("jp", "wss://jp.example", "abc").is_err());
    }

    #[test]
    fn region_commands_parse() {
        let cli = crate::Cli::try_parse_from([
            "kuzz-admin",
            "regions",
            "add",
            "--id",
            "jp",
            "--connect-url",
            "wss://jp.example",
            "--link-pubkey",
            "ab",
        ]);
        assert!(cli.is_ok());
        assert!(crate::Cli::try_parse_from(["kuzz-admin", "regions", "list"]).is_ok());
        assert!(crate::Cli::try_parse_from(["kuzz-admin", "regions", "remove"]).is_err());
    }
}
