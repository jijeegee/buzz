//! Attachment permissions against a real database: who may open a blob after
//! uploads, references, membership changes and deletions.
use chrono::{DateTime, Utc};
use kuzz_core::CommunityId;
use nostr::{Event, EventBuilder, Keys, Kind, Tag};
use sqlx::PgPool;
use uuid::Uuid;

use crate::channel::{ChannelType, ChannelVisibility, MemberRole};
use crate::{channel, channel_members, Db};

const SHA: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const ICON_SHA: &str = "2222222222222222222222222222222222222222222222222222222222222222";

struct Fixture {
    pool: PgPool,
    db: Db,
    scope: CommunityId,
}

fn pk(keys: &Keys) -> Vec<u8> {
    keys.public_key().to_bytes().to_vec()
}

impl Fixture {
    async fn new() -> Self {
        let pool = PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("isolated test database");
        if std::env::var("KUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate");
        }
        let scope = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id, host) VALUES ($1, $2)")
            .bind(scope.as_uuid())
            .bind(format!("media-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        Self {
            db: Db::from_pool(pool.clone()),
            pool,
            scope,
        }
    }

    async fn room(&self, visibility: ChannelVisibility, owner: &Keys, members: &[&Keys]) -> Uuid {
        let id = channel::create_channel(
            &self.pool,
            self.scope,
            "room",
            ChannelType::Stream,
            visibility,
            None,
            &pk(owner),
            None,
        )
        .await
        .expect("channel")
        .id;
        // The creator is already the owner.
        for member in members {
            channel_members::add_member(
                &self.pool,
                self.scope,
                id,
                &pk(member),
                MemberRole::Member,
                Some(&pk(owner)),
            )
            .await
            .expect("member");
        }
        id
    }

    /// Store an event and link `SHA` to it, as ingest does.
    async fn reference(&self, room: Option<Uuid>, keys: &Keys) -> Event {
        let mut builder = EventBuilder::new(Kind::Custom(9), "photo");
        if let Some(room) = room {
            builder = builder.tags([Tag::parse(["h", &room.to_string()]).expect("h tag")]);
        }
        let event = builder.sign_with_keys(keys).expect("signature");
        self.db
            .insert_event(self.scope, &event, room)
            .await
            .expect("event write");
        let created_at: DateTime<Utc> =
            DateTime::from_timestamp(event.created_at.as_secs() as i64, 0).unwrap();
        self.db
            .record_media_references(
                self.scope,
                &event.id.to_bytes(),
                created_at,
                room,
                &[SHA.to_owned()],
            )
            .await
            .expect("link");
        event
    }

    async fn readable(&self, reader: &Keys) -> bool {
        self.db
            .media_readable_by(self.scope, &pk(reader), SHA)
            .await
            .expect("access check")
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn private_channel_attachment_follows_current_membership() {
    let f = Fixture::new().await;
    let (uploader, member, outsider, late) = (
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
    );

    f.db.record_media_upload(f.scope, SHA, &pk(&uploader))
        .await
        .expect("upload record");
    assert!(
        f.readable(&uploader).await,
        "the uploader can preview before sending"
    );
    assert!(
        !f.readable(&member).await,
        "nobody else can before it is sent"
    );

    let room = f
        .room(ChannelVisibility::Private, &uploader, &[&member])
        .await;
    let message = f.reference(Some(room), &uploader).await;
    assert!(f.readable(&member).await, "a member can open it once sent");
    assert!(!f.readable(&outsider).await, "a non-member cannot");

    channel_members::add_member(
        &f.pool,
        f.scope,
        room,
        &pk(&late),
        MemberRole::Member,
        Some(&pk(&uploader)),
    )
    .await
    .expect("late join");
    assert!(
        f.readable(&late).await,
        "someone who joins later can open it"
    );

    sqlx::query(
        "UPDATE channel_members SET removed_at = now() \
         WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3",
    )
    .bind(f.scope.as_uuid())
    .bind(room)
    .bind(pk(&member))
    .execute(&f.pool)
    .await
    .expect("remove member");
    assert!(!f.readable(&member).await, "a removed member loses access");

    sqlx::query("UPDATE events SET deleted_at = now() WHERE community_id = $1 AND id = $2")
        .bind(f.scope.as_uuid())
        .bind(message.id.to_bytes().as_slice())
        .execute(&f.pool)
        .await
        .expect("delete message");
    assert!(
        !f.readable(&late).await,
        "a deleted message unlocks nothing"
    );
    assert!(
        f.readable(&uploader).await,
        "the uploader always keeps access"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn open_channels_community_events_and_icon_are_readable_by_members() {
    let f = Fixture::new().await;
    let (author, anyone) = (Keys::generate(), Keys::generate());

    let room = f.room(ChannelVisibility::Open, &author, &[]).await;
    f.reference(Some(room), &author).await;
    assert!(
        f.readable(&anyone).await,
        "open channel attachments are public to members"
    );

    let f = Fixture::new().await;
    f.reference(None, &author).await;
    assert!(
        f.readable(&anyone).await,
        "community-wide events (profiles) are readable"
    );

    sqlx::query("UPDATE communities SET icon = $2 WHERE id = $1")
        .bind(f.scope.as_uuid())
        .bind(format!("https://relay.example/media/{ICON_SHA}.png"))
        .execute(&f.pool)
        .await
        .expect("icon");
    assert!(f
        .db
        .media_readable_by(f.scope, &pk(&anyone), ICON_SHA)
        .await
        .expect("icon access"));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn deleted_channel_attachments_are_closed() {
    let f = Fixture::new().await;
    let (author, member) = (Keys::generate(), Keys::generate());
    let room = f.room(ChannelVisibility::Open, &author, &[&member]).await;
    f.reference(Some(room), &author).await;
    assert!(f.readable(&member).await);

    sqlx::query("UPDATE channels SET deleted_at = now() WHERE community_id = $1 AND id = $2")
        .bind(f.scope.as_uuid())
        .bind(room)
        .execute(&f.pool)
        .await
        .expect("delete channel");
    assert!(!f.readable(&member).await);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn an_agent_can_use_what_its_owner_uploaded() {
    let f = Fixture::new().await;
    let (owner, agent, stranger) = (Keys::generate(), Keys::generate(), Keys::generate());
    for keys in [&owner, &agent, &stranger] {
        f.db.ensure_user(f.scope, keys.public_key().as_bytes())
            .await
            .expect("user");
    }
    f.db.set_agent_owner(
        f.scope,
        agent.public_key().as_bytes(),
        owner.public_key().as_bytes(),
    )
    .await
    .expect("owner link");
    f.db.record_media_upload(f.scope, SHA, &pk(&owner))
        .await
        .expect("upload record");
    assert!(f.readable(&agent).await, "the owner's agent can publish it");
    assert!(!f.readable(&stranger).await);
}

impl Fixture {
    async fn in_use(&self, grace_cutoff: DateTime<Utc>) -> bool {
        !self
            .db
            .media_hashes_in_use(self.scope, &[SHA.to_owned()], grace_cutoff)
            .await
            .expect("cleanup check")
            .is_empty()
    }

    /// Store an event that names `SHA` in its content without linking it.
    async fn naming_event(&self, kind: u16, keys: &Keys) -> Event {
        let event = EventBuilder::new(
            Kind::Custom(kind),
            format!("https://x.test/media/{SHA}.png"),
        )
        .sign_with_keys(keys)
        .expect("signature");
        self.db
            .insert_event(self.scope, &event, None)
            .await
            .expect("event write");
        event
    }

    async fn delete_event_at(&self, event: &Event, at: DateTime<Utc>) {
        sqlx::query("UPDATE events SET deleted_at = $3 WHERE community_id = $1 AND id = $2")
            .bind(self.scope.as_uuid())
            .bind(event.id.to_bytes().to_vec())
            .bind(at)
            .execute(&self.pool)
            .await
            .expect("delete event");
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn cleanup_keeps_recent_uploads_and_drops_old_unused_ones() {
    let f = Fixture::new().await;
    let alice = Keys::generate();
    let now = Utc::now();
    let cutoff = now - chrono::Duration::days(7);
    assert!(!f.in_use(cutoff).await, "never uploaded or named");

    f.db.record_media_upload(f.scope, SHA, &pk(&alice))
        .await
        .expect("upload");
    assert!(f.in_use(cutoff).await, "uploaded within the grace period");
    sqlx::query("UPDATE media_uploads SET created_at = $2 WHERE community_id = $1")
        .bind(f.scope.as_uuid())
        .bind(now - chrono::Duration::days(8))
        .execute(&f.pool)
        .await
        .expect("age upload");
    assert!(!f.in_use(cutoff).await, "old upload no message uses");

    // Attaching the same bytes again restarts the grace period.
    f.db.record_media_upload(f.scope, SHA, &pk(&alice))
        .await
        .expect("re-upload");
    assert!(
        f.in_use(cutoff).await,
        "re-uploaded within the grace period"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn cleanup_keeps_attachments_of_reported_messages() {
    let f = Fixture::new().await;
    let alice = Keys::generate();
    let now = Utc::now();
    let cutoff = now - chrono::Duration::days(7);
    let message = f.naming_event(9, &alice).await;
    f.delete_event_at(&message, now - chrono::Duration::days(30))
        .await;
    assert!(!f.in_use(cutoff).await, "deleted long ago");

    sqlx::query(
        "INSERT INTO moderation_reports \
           (community_id, report_event_id, reporter_pubkey, target_kind, target_event_id, report_type) \
         VALUES ($1, $2, $3, 'event', $4, 'other')",
    )
    .bind(f.scope.as_uuid())
    .bind(vec![7u8; 32])
    .bind(pk(&Keys::generate()))
    .bind(message.id.to_bytes().to_vec())
    .execute(&f.pool)
    .await
    .expect("report");
    assert!(
        f.in_use(cutoff).await,
        "reported message keeps its attachment"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn cleanup_keeps_files_any_live_or_recently_deleted_event_names() {
    let f = Fixture::new().await;
    let alice = Keys::generate();
    let now = Utc::now();
    let cutoff = now - chrono::Duration::days(7);

    // A private persona is never linked for readers but still uses the file.
    let persona = f.naming_event(30175, &alice).await;
    assert!(f.in_use(cutoff).await, "live private event");
    f.delete_event_at(&persona, now - chrono::Duration::days(1))
        .await;
    assert!(f.in_use(cutoff).await, "deleted within the grace period");
    f.delete_event_at(&persona, now - chrono::Duration::days(8))
        .await;
    assert!(!f.in_use(cutoff).await, "deleted long ago");

    // A linked message in a room keeps it while live.
    let room = f.room(ChannelVisibility::Private, &alice, &[]).await;
    let message = f.reference(Some(room), &alice).await;
    assert!(f.in_use(cutoff).await, "linked live message");
    f.delete_event_at(&message, now - chrono::Duration::days(8))
        .await;
    assert!(!f.in_use(cutoff).await, "every message deleted long ago");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn cleanup_keeps_profile_pictures_icons_and_reported_files() {
    let f = Fixture::new().await;
    let alice = Keys::generate();
    let cutoff = Utc::now() - chrono::Duration::days(7);
    let url = format!("https://x.test/media/{SHA}.png");

    crate::user::ensure_user(&f.pool, f.scope, &pk(&alice))
        .await
        .expect("user");
    sqlx::query("UPDATE users SET avatar_url = $3 WHERE community_id = $1 AND pubkey = $2")
        .bind(f.scope.as_uuid())
        .bind(pk(&alice))
        .bind(&url)
        .execute(&f.pool)
        .await
        .expect("avatar");
    assert!(f.in_use(cutoff).await, "profile picture");
    sqlx::query("UPDATE users SET avatar_url = NULL WHERE community_id = $1")
        .bind(f.scope.as_uuid())
        .execute(&f.pool)
        .await
        .expect("clear avatar");
    assert!(!f.in_use(cutoff).await);

    sqlx::query("UPDATE communities SET icon = $2 WHERE id = $1")
        .bind(f.scope.as_uuid())
        .bind(&url)
        .execute(&f.pool)
        .await
        .expect("icon");
    assert!(f.in_use(cutoff).await, "community icon");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn forget_media_drops_only_stale_rows_of_that_file() {
    let f = Fixture::new().await;
    let alice = Keys::generate();
    let room = f.room(ChannelVisibility::Open, &alice, &[]).await;
    f.reference(Some(room), &alice).await;
    f.db.record_media_upload(f.scope, SHA, &pk(&alice))
        .await
        .expect("upload");
    f.db.record_media_upload(f.scope, ICON_SHA, &pk(&alice))
        .await
        .expect("other upload");
    let rows = || async {
        let mut rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT 'upload', sha256 FROM media_uploads WHERE community_id = $1 \
             UNION ALL SELECT 'reference', sha256 FROM media_references WHERE community_id = $1",
        )
        .bind(f.scope.as_uuid())
        .fetch_all(&f.pool)
        .await
        .expect("rows");
        rows.sort();
        rows
    };

    // A fresh upload stays.
    f.db.forget_media(f.scope, SHA, Utc::now() - chrono::Duration::days(7))
        .await
        .expect("forget");
    assert_eq!(rows().await.len(), 3);

    // An old upload goes; the live message's reference and the other file stay.
    f.db.forget_media(f.scope, SHA, Utc::now() + chrono::Duration::days(1))
        .await
        .expect("forget");
    assert_eq!(
        rows().await,
        vec![
            ("reference".to_owned(), SHA.to_owned()),
            ("upload".to_owned(), ICON_SHA.to_owned()),
        ]
    );
}
