//! Attachment permissions: a blob is readable by its uploaders and by whoever
//! can currently read an event that references it (see
//! `kuzz_db::media_access`).
//!
//! Stored events link the blobs they name: imeta `x <sha256>` entries and any
//! `/media/<sha256>` URL in the content or tags (attachments, thumbnails,
//! video posters, link-preview images, custom emoji, profile pictures). A link
//! is recorded only when the author could already open the blob, so pasting
//! someone else's hash into a new event unlocks nothing. Relay-signed events
//! are checked against the person they speak for (`actor` / `p`), and trusted
//! only when they are the relay's own. Events no other member can read link
//! nothing. Linking is idempotent, so a client retry of a stored event links
//! again.

use std::collections::BTreeSet;
use std::time::Duration;

use kuzz_core::CommunityId;
use kuzz_db::media_access::MAX_MEDIA_HASHES_PER_EVENT;
use nostr::Event;
use tracing::{debug, error};
use uuid::Uuid;

use crate::state::AppState;

/// How long a positive read answer is reused.
pub const READ_GRANT_TTL: Duration = Duration::from_secs(60);

const MEDIA_PATH: &str = "/media/";

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

fn collect_media_urls(text: &str, out: &mut BTreeSet<String>) {
    let mut rest = text;
    while let Some(at) = rest.find(MEDIA_PATH) {
        rest = &rest[at + MEDIA_PATH.len()..];
        let Some(candidate) = rest.get(..64) else {
            return;
        };
        let ends_cleanly = !rest[64..].starts_with(|c: char| c.is_ascii_hexdigit());
        if is_sha256_hex(candidate) && ends_cleanly {
            out.insert(candidate.to_owned());
        }
    }
}

/// Blob hashes an event names, at most [`MAX_MEDIA_HASHES_PER_EVENT`].
pub fn referenced_media_hashes(event: &Event) -> Vec<String> {
    let mut hashes = BTreeSet::new();
    collect_media_urls(&event.content, &mut hashes);
    for tag in event.tags.iter() {
        let parts = tag.as_slice();
        let is_imeta = parts.first().map(String::as_str) == Some("imeta");
        for part in parts.iter().skip(1) {
            collect_media_urls(part, &mut hashes);
            if is_imeta {
                if let Some(sha) = part.strip_prefix("x ").filter(|s| is_sha256_hex(s)) {
                    hashes.insert(sha.to_owned());
                }
            }
        }
    }
    hashes
        .into_iter()
        .take(MAX_MEDIA_HASHES_PER_EVENT)
        .collect()
}

/// Whether members other than the author (and p-tagged owners) can read the
/// event at all. Reuses the read path's result gate with a reader that matches
/// nobody.
fn readable_beyond_author(event: &Event) -> bool {
    crate::handlers::req::event_visible_to_reader(event, &[0u8; 32])
}

/// Link the blobs a freshly stored event names, before it fans out so its
/// readers can open them at once. Failures are logged, never surfaced: the
/// event is already stored, and its blobs stay readable by their uploaders.
pub async fn record_event_media(
    state: &AppState,
    community: CommunityId,
    event: &Event,
    channel_id: Option<Uuid>,
) {
    let hashes = referenced_media_hashes(event);
    if hashes.is_empty() || !readable_beyond_author(event) {
        return;
    }
    // A relay-signed event on someone's behalf (workflow message) is checked
    // against that person; one that is purely the relay's own is trusted.
    let relay = state.relay_keypair.public_key();
    let author = crate::handlers::ingest::effective_message_author(event, &relay);
    let linkable = if author == relay.to_bytes() {
        hashes
    } else {
        match state
            .db
            .media_hashes_readable_by(community, &author, &hashes)
            .await
        {
            Ok(readable) => {
                if readable.len() < hashes.len() {
                    debug!(
                        event_id = %event.id,
                        skipped = hashes.len() - readable.len(),
                        "event names media its author cannot open; not linked"
                    );
                }
                readable
            }
            Err(e) => {
                error!(event_id = %event.id, "media link author check failed: {e}");
                return;
            }
        }
    };
    if linkable.is_empty() {
        return;
    }
    let Some(created_at) = chrono::DateTime::from_timestamp(event.created_at.as_secs() as i64, 0)
    else {
        return;
    };
    if let Err(e) = state
        .db
        .record_media_references(
            community,
            &event.id.to_bytes(),
            created_at,
            channel_id,
            &linkable,
        )
        .await
    {
        error!(event_id = %event.id, "recording media links failed: {e}");
    }
}

/// Whether `reader` may open the blob `sha256` right now.
pub async fn reader_may_open(
    state: &AppState,
    community: CommunityId,
    reader: &[u8],
    sha256: &str,
) -> Result<bool, kuzz_db::DbError> {
    let key = (community, reader.to_vec(), sha256.to_owned());
    if state.media_read_grants.contains_key(&key) {
        return Ok(true);
    }
    let readable = state
        .db
        .media_readable_by(community, reader, sha256)
        .await?;
    if readable {
        state.media_read_grants.insert(key, ());
    }
    Ok(readable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const SHA_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const SHA_D: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

    fn event(kind: u16, content: &str, tags: Vec<Vec<String>>) -> Event {
        EventBuilder::new(Kind::Custom(kind), content)
            .tags(tags.into_iter().map(|t| Tag::parse(t).unwrap()))
            .sign_with_keys(&Keys::generate())
            .unwrap()
    }

    fn strings(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn finds_imeta_hashes_urls_thumbnails_and_profile_pictures() {
        let message = event(
            9,
            &format!("look https://relay.example/media/{SHA_A}.jpg"),
            vec![
                strings(&[
                    "imeta",
                    &format!("url https://relay.example/media/{SHA_A}.jpg"),
                    &format!("x {SHA_A}"),
                    &format!("thumb https://relay.example/media/{SHA_B}.thumb.jpg"),
                ]),
                strings(&[
                    "emoji",
                    "party",
                    &format!("https://relay.example/media/{SHA_C}.png"),
                ]),
            ],
        );
        assert_eq!(
            referenced_media_hashes(&message),
            strings(&[SHA_A, SHA_B, SHA_C])
        );

        let profile = event(
            0,
            &format!(r#"{{"name":"a","picture":"https://relay.example/media/{SHA_D}.webp"}}"#),
            vec![],
        );
        assert_eq!(referenced_media_hashes(&profile), strings(&[SHA_D]));
    }

    #[test]
    fn ignores_event_ids_and_malformed_hashes() {
        let reply = event(
            9,
            &format!("/media/{SHA_A}ff /media/{} /media/", &SHA_B[..63]),
            vec![
                strings(&["e", SHA_C]),
                strings(&["p", SHA_D]),
                strings(&["t", &format!("x {SHA_D}")]),
            ],
        );
        assert!(referenced_media_hashes(&reply).is_empty());
    }

    #[test]
    fn private_events_link_nothing() {
        let shared_off = event(30175, "", vec![strings(&["d", "p"])]);
        assert!(!readable_beyond_author(&shared_off));
        let shared_on = event(
            30175,
            "",
            vec![strings(&["d", "p"]), strings(&["shared", "true"])],
        );
        assert!(readable_beyond_author(&shared_on));
        let reminder = event(30300, "", vec![strings(&["d", "r"])]);
        assert!(!readable_beyond_author(&reminder));
        assert!(readable_beyond_author(&event(9, "hi", vec![])));
    }

    mod postgres_tests {
        use super::*;
        use kuzz_db::channel::{ChannelType, ChannelVisibility, MemberRole};

        struct World {
            state: std::sync::Arc<AppState>,
            community: CommunityId,
        }

        impl World {
            async fn new() -> Self {
                let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
                let pool = sqlx::PgPool::connect(&url).await.expect("connect");
                let community = CommunityId::from_uuid(Uuid::new_v4());
                sqlx::query("INSERT INTO communities(id, host) VALUES ($1, $2)")
                    .bind(community.as_uuid())
                    .bind(format!("media-{}.test", community.as_uuid()))
                    .execute(&pool)
                    .await
                    .expect("community");
                let state = crate::state::tests::test_state_with_database_pool(pool).await;
                Self { state, community }
            }

            async fn private_room(&self, members: &[&Keys]) -> Uuid {
                let room = self
                    .state
                    .db
                    .create_channel(
                        self.community,
                        "room",
                        ChannelType::Stream,
                        ChannelVisibility::Private,
                        None,
                        &members[0].public_key().to_bytes(),
                        None,
                    )
                    .await
                    .expect("room")
                    .id;
                // The creator is already the owner.
                for member in &members[1..] {
                    self.state
                        .db
                        .add_member(
                            self.community,
                            room,
                            &member.public_key().to_bytes(),
                            MemberRole::Member,
                            Some(&members[0].public_key().to_bytes()),
                        )
                        .await
                        .expect("member");
                }
                room
            }

            /// Store a message naming `SHA_A` and link it the way ingest does.
            async fn post(&self, room: Uuid, author: &Keys) {
                let event = EventBuilder::new(Kind::Custom(9), "photo")
                    .tags([
                        Tag::parse(["h", &room.to_string()]).unwrap(),
                        Tag::parse(["imeta", &format!("x {SHA_A}")]).unwrap(),
                    ])
                    .sign_with_keys(author)
                    .unwrap();
                self.state
                    .db
                    .insert_event(self.community, &event, Some(room))
                    .await
                    .expect("store");
                record_event_media(&self.state, self.community, &event, Some(room)).await;
            }

            async fn can_open(&self, reader: &Keys) -> bool {
                self.state
                    .db
                    .media_readable_by(self.community, &reader.public_key().to_bytes(), SHA_A)
                    .await
                    .expect("access check")
            }
        }

        #[tokio::test]
        #[ignore = "requires Postgres"]
        async fn naming_someone_elses_hash_unlocks_nothing_but_forwarding_works() {
            let w = World::new().await;
            let (owner, member, thief, accomplice, friend) = (
                Keys::generate(),
                Keys::generate(),
                Keys::generate(),
                Keys::generate(),
                Keys::generate(),
            );
            w.state
                .db
                .record_media_upload(w.community, SHA_A, &owner.public_key().to_bytes())
                .await
                .expect("upload");
            let team = w.private_room(&[&owner, &member]).await;
            w.post(team, &owner).await;
            assert!(w.can_open(&member).await);

            // The thief learned the hash and posts it in their own room.
            let hideout = w.private_room(&[&thief, &accomplice]).await;
            w.post(hideout, &thief).await;
            assert!(!w.can_open(&thief).await, "naming the hash is not access");
            assert!(!w.can_open(&accomplice).await, "nor for the thief's room");

            // A member who can see it forwards it to another room.
            let side = w.private_room(&[&member, &friend]).await;
            w.post(side, &member).await;
            assert!(w.can_open(&friend).await, "a forward reaches the new room");
        }
    }
}
