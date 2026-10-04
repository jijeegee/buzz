# Centralized Identity — 설계 및 단계별 구현 계획

상태: v4 (2026-10-04, 리뷰어 READY). v1 리뷰(B1–B4) + 사용자 확정 사항 + v2 리뷰(B5–B8) + v3 선택 항목 반영.
범위: Buzz 포크를 "중앙 서버 계정(Google 로그인) + 토큰 인증" 모델로 전환하는 설계와 구현 계획.
원칙: 결정되지 않은 부분은 KakaoTalk/Telegram이 하는 방식을 따른다.

목차
1. 목표 / 비목표 / 제거 항목
2. 아이덴티티 모델, 데이터 모델, 토큰, 폐기 매트릭스
3. API / 프로토콜
4. 컴포넌트별 변경 목록
5. 단계별 계획 (Phase 0–4)
6. 보안 분석
7. 확정 사항
8. v1 → v2 변경 요약

---

## 1. 목표 / 비목표 / 제거 항목

### 1.1 목표

| # | 목표 |
|---|------|
| G1 | 아이덴티티 = 서버 계정. `user`와 `bot`이 principal이며 서버가 발급한 32바이트 principal id로 식별된다. 토큰은 자격증명일 뿐 아이덴티티가 아니다. |
| G2 | 사람: **Google 계정 로그인(OIDC)** → 디바이스별 세션. access token 1h, refresh token 90일 sliding, refresh는 OS keychain 보관. 디바이스 목록과 원격 로그아웃 제공. 비밀번호 없음. Apple 등 provider 추가 가능한 구조. |
| G3 | Desktop 호스팅 봇: 서버에 `bot id`로 등록(owner = user, host = device). Desktop이 봇 시작 시 1h bot access token을 받아 `BUZZ_BOT_TOKEN` env로 buzz-acp에 주입, 디스크 미기록. acp는 만료 전 "exchange while valid"로 자체 갱신하고 같은 WS 연결에서 재-AUTH한다. |
| G4 | 헤드리스 봇(헤르홈류): 서버가 발급한 장기 bot token, 회수/교체 가능, 같은 bot id → 히스토리 유지. |
| G5 | buzz-acp ↔ Desktop 프로세스 관계는 오늘과 동일(자식 프로세스, Desktop 종료 시 함께 종료). acp와 `buzz` CLI는 여전히 서버에 직접 접속하되 서명 대신 토큰을 쓴다. |
| G6 | 서버가 `pubkey`(= sender principal id)를 스탬프한다. 클라이언트 서명, `BUZZ_PRIVATE_KEY`, `BUZZ_AUTH_TAG`, Nostr 호환은 제거한다. |
| G7 | 프로필(이름/아바타)은 Telegram처럼 **계정 전역**이다. 커뮤니티별 프로필 없음. |
| G8 | 모든 중간 상태가 빌드·테스트 가능해야 한다 (`just ci`, `just test`). |

### 1.2 비목표

- 비밀번호/매직링크 로그인. Google OIDC만. (provider 테이블 구조만 열어둔다.)
- E2E 암호화. VISION.md:102 "server-managed encryption, no E2E"와 일치.
- Mesh/sovereign(VISION_MESH.md, VISION_SOVEREIGN.md) 유지. 포기한다.
- 기존 managed agent의 히스토리 보존 마이그레이션. 단절을 수용한다(확정 Q7).
- Hermes 프로필 기능(`feat/hermes-per-agent-profile`, 47d3f23d2) 변경. §4.13에서 env 목록만 보강.

### 1.3 명시적 제거 항목과 수용 근거

| 제거 | 현재 위치 | 수용 근거 |
|------|-----------|-----------|
| 클라이언트 Schnorr 서명 / `sig` 검증 | `buzz-core/src/verification.rs`, `buzz-relay/src/handlers/ingest.rs:2407` | 서버가 authoritative 저장소. 서버 인증이 sender 보증을 대신한다. |
| NIP-42 WS 인증 (kind 22242) | `buzz-auth/src/nip42.rs`, `buzz-relay/src/handlers/auth.rs`, `connection.rs:526`, `audio/handler.rs:495-576` | 토큰 AUTH 프레임으로 대체. |
| NIP-98 HTTP 인증 (kind 27235) | `buzz-auth/src/nip98.rs`, `api/bridge.rs:98-176`, `api/admin/auth.rs:371-434`, `api/git/transport.rs:160-180`, `nip98.rs`, `web/src/shared/lib/nip98.ts` | `Authorization: Bearer`로 대체. |
| NIP-OA owner attestation (`auth` 태그, `BUZZ_AUTH_TAG`) | `buzz-sdk/src/nip_oa.rs`, `handlers/auth.rs:288`, desktop `runtime.rs:773`, git 크레이트 | 소유 관계가 서버 `bots.owner_principal_id`로 이동. |
| NIP-FI federated identity | `buzz-auth/src/nip_fi/*`, `buzz-relay/src/nip_fi_*.rs`, `api/nip_fi.rs`, migrations 0041/0042 | 새 계정 모델이 완전히 대체. 세션 만료 타이머 패턴만 재사용. |
| `api_tokens`, `pubkey_allowlist`, `relay_invites`의 NIP-98 면제 경로 | `schema.sql`, `buzz-db/src/store/{api_token,allowlist}.rs` | 새 `access_tokens`/`sessions`로 대체. |
| 키 백업·nsec import/export·HPKE 백업 | desktop `commands/identity.rs:242-587`, `key_backup.rs`, `hpke_key_backup.rs` | 계정이 서버(Google)에 있으므로 "키 분실"이라는 개념이 없다. |
| `git-sign-nostr`, `git-credential-nostr`의 키 로딩 | `crates/git-sign-nostr`, `crates/git-credential-nostr/src/lib.rs:50-105` | push 인증은 토큰. 커밋 서명은 사용자 GPG/SSH. |
| NIP-17/NIP-44 gift wrap 암호화 레이어 (kind 1059) | `handlers/event.rs:653`, mobile `shared/crypto/{nip44,ecdh,nip_oa}.dart` | **DM 모델은 무변경**(오늘처럼 `h` 스코프 비공개 채널 + kind 30622 가시성). 암호화 레이어만 제거 → 서버 저장 평문(확정 Q4). |
| NIP-AB 페어링, `buzz-pair-relay`, `buzz-pairing-cli`, mesh 크레이트/데모 | 해당 크레이트, desktop `nostr_bind.rs`, `api/mesh_demo.rs`, `audio/mesh.rs` | 디바이스 추가는 Google 로그인으로 대체(확정 Q8). |
| 커뮤니티별 프로필(`users.display_name` 등 source of truth) | `users` 테이블, kind 0 per community | 전역 프로필(확정 Q2). `users`는 멤버십/권한 projection으로만 남는다. |

### 1.4 VISION과의 긴장 (명시)

VISION.md Identity 절은 "사람과 에이전트 모두 secp256k1 키쌍, NIP-05, 'The conversation is Nostr', 'Identity is portable'"을 선언하고, VISION_SOVEREIGN.md·VISION_MESH.md는 Nostr 아이덴티티를 기둥으로 삼는다. 이 계획은 그 기둥을 의도적으로 뺀다.

- 포기: 아이덴티티 이식성, 릴레이 간 연합, 제3자 검증 가능한 감사 체인, 메시 오프라인 동작, 셀프 호스팅 사용자가 Google 없이 가입하는 경로.
- 유지: 이벤트 모델(kind/tags/`h` 스코핑), 채널/스레드/워크플로우/에이전트 하네스, 서버 authoritative 저장.
- 결론: VISION.md Identity 절은 Phase 4에서 다시 쓴다. 상류 Buzz와의 머지 가능성은 사라진다는 점을 사용자가 수용했다.

---

## 2. 아이덴티티 모델 · 데이터 모델 · 토큰 · 폐기 매트릭스

### 2.1 Principal

| 종류 | id | 소유 | 자격증명 |
|------|----|------|----------|
| `user` | 32바이트, hex 64자 | 본인 (Google 계정 1개 = 계정 1개) | Google OIDC → 세션(refresh/access) |
| `bot` | 32바이트 | `owner_principal_id` (user) | `bzb_` (Desktop 호스팅, 1h) 또는 `bzk_` (headless, 장기) |
| `relay` | 배포당 1개 row | 운영자 | 없음. 오늘의 `relay_keypair.public_key()` 역할(서버 발행 이벤트의 sender). |

**principal id 불변식 (B1 해결).** principal id는 32바이트 "아무 랜덤값"이 아니라 **유효한 secp256k1 x-only 공개키**여야 한다. 코드베이스 전체가 `nostr::PublicKey::from_slice/from_hex`로 파싱한다: `buzz-db/src/store/event.rs:881-914 row_to_stored_event`(실패 시 `Ok(None)`으로 행이 사라짐), `handlers/req.rs`의 `authors` 파싱, `AuthContext.pubkey`, desktop의 `PublicKey::from_hex` 82곳, mobile `NostrEvent`. 랜덤 32바이트의 약 절반은 유효한 x좌표가 아니므로 메시지 절반이 증발한다.

- 생성: `buzz_core::principal::PrincipalId::generate()` = `nostr::Keys::generate().public_key()`에서 공개키만 취하고 비밀키는 즉시 drop. 비밀키는 어디에도 저장·전달되지 않는다(그래서 "키 없음" 모델은 유지된다).
- 검증: `buzz-db` `create_principal`은 INSERT 전에 `PublicKey::from_slice`를 통과한 값만 받는다(`PrincipalId` newtype이 생성자에서 보장, 임의 바이트에서의 변환은 `TryFrom`으로만). SQL `CHECK(length(id)=32)`는 유지하되 secp 유효성은 SQL로 검사할 수 없으므로 스토어 테스트가 이 seam을 고정한다.
- 이 불변식은 Phase 4에서 `BuzzEvent`/`PrincipalId` 타입 교체가 끝나 `nostr::PublicKey` 파싱이 사라질 때까지 유지한다(§3.4 참조). 교체 후에도 이미 발급된 id는 그대로 유효하다.

### 2.2 테이블 (신규, 배포 전역)

신규 테이블은 커뮤니티(테넌트) 전역이다. 오늘의 pubkey가 전역이고 `users`가 커뮤니티별 row인 구조를 그대로 따른다. 멀티테넌트 lint의 `_operator_global_tables` 등록이 필요하다(`migrations/0055` 패턴). `schema/schema.sql` desired state도 같은 PR에서 갱신한다.

```sql
-- 0056_principals.sql
CREATE TABLE principals (
    id            BYTEA PRIMARY KEY CHECK (length(id) = 32),
    kind          TEXT NOT NULL CHECK (kind IN ('user','bot','relay')),
    display_name  TEXT NOT NULL DEFAULT '',
    avatar_url    TEXT,
    username      TEXT,                        -- Telegram식 선택 @username. NULL 허용
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at   TIMESTAMPTZ,
    purge_after   TIMESTAMPTZ                  -- 계정 삭제 예약(§2.5)
);
CREATE UNIQUE INDEX principals_username_lower ON principals (lower(username)) WHERE username IS NOT NULL;
CREATE UNIQUE INDEX principals_single_relay  ON principals ((kind)) WHERE kind = 'relay';  -- 배포당 1개 (B1)

-- 외부 신원. provider 추가 = row 추가 (Apple 등). (provider, subject)가 Google의 `sub`.
CREATE TABLE identities (
    provider      TEXT NOT NULL CHECK (provider IN ('google')),  -- Apple 추가 시 CHECK 확장 마이그레이션
    subject       TEXT NOT NULL,
    principal_id  BYTEA NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    email         TEXT,                        -- 표시용. 로그인 키는 (provider, subject)
    linked_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at TIMESTAMPTZ,
    PRIMARY KEY (provider, subject)
);
CREATE INDEX identities_principal ON identities (principal_id);

CREATE TABLE devices (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    principal_id  BYTEA NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    platform      TEXT NOT NULL CHECK (platform IN ('desktop','mobile','web','cli')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at    TIMESTAMPTZ
);

CREATE TABLE sessions (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    device_id         UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_refreshed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at        TIMESTAMPTZ,
    revoked_reason    TEXT
);

-- refresh 토큰은 세션당 여러 row (rotation 이력). 재사용 탐지의 근거 (B2).
CREATE TABLE refresh_tokens (
    token_hash    BYTEA PRIMARY KEY CHECK (length(token_hash) = 32),
    session_id    UUID NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    generation    INT  NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at    TIMESTAMPTZ NOT NULL,        -- now()+90d (sliding: 새 row마다 갱신)
    used_at       TIMESTAMPTZ                  -- rotation으로 소비된 시각. NULL = 현재 유효 row
);
CREATE UNIQUE INDEX refresh_tokens_live ON refresh_tokens (session_id) WHERE used_at IS NULL;  -- 세션당 live 1개
CREATE INDEX refresh_tokens_session ON refresh_tokens (session_id);

CREATE TABLE bots (
    id                 BYTEA PRIMARY KEY REFERENCES principals(id) ON DELETE CASCADE,
    owner_principal_id BYTEA NOT NULL REFERENCES principals(id),
    host_device_id     UUID REFERENCES devices(id) ON DELETE SET NULL,   -- NULL = headless
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at         TIMESTAMPTZ
);
CREATE INDEX bots_owner ON bots (owner_principal_id) WHERE deleted_at IS NULL;

CREATE TABLE access_tokens (
    token_hash       BYTEA PRIMARY KEY CHECK (length(token_hash) = 32),
    principal_id     BYTEA NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    kind             TEXT NOT NULL CHECK (kind IN ('user','bot','bot_headless')),
    session_id       UUID REFERENCES sessions(id) ON DELETE CASCADE,     -- kind=user
    bot_id           BYTEA REFERENCES bots(id) ON DELETE CASCADE,        -- kind=bot*
    issued_by_device UUID REFERENCES devices(id) ON DELETE CASCADE,      -- kind=bot (host)
    expires_at       TIMESTAMPTZ,              -- NULL = headless 장기
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at     TIMESTAMPTZ,
    superseded_at    TIMESTAMPTZ,              -- exchange로 대체된 시각(60초 grace 시작). 설정되면 exchange 불가(§6.5)
    revoked_at       TIMESTAMPTZ,
    revoked_reason   TEXT,                     -- 'exchanged' | 'reissued' | 'stopped' | 'logout' | 'device_revoked' | 'bot_deleted' | 'account_disabled' | 'revoke_all'
    CHECK ((kind='user' AND session_id IS NOT NULL AND bot_id IS NULL)
        OR (kind<>'user' AND bot_id IS NOT NULL AND session_id IS NULL))
);
CREATE INDEX access_tokens_principal ON access_tokens (principal_id) WHERE revoked_at IS NULL;
CREATE INDEX access_tokens_session   ON access_tokens (session_id)   WHERE revoked_at IS NULL;
CREATE INDEX access_tokens_bot       ON access_tokens (bot_id)       WHERE revoked_at IS NULL;
```

**운영자 테이블은 신설하지 않는다 (B5).** `relay_operators`는 이미 존재한다(`schema.sql:1812`, migration 0035: `pubkey BYTEA PK(32)`, `role IN ('operator','moderator')`, `added_by`, `created_at`; 스토어 `buzz-db/src/store/relay_operators.rs`; `api/admin/auth.rs`의 `AdminRole`이 DB 로스터 + config 우선순위로 해석). `pubkey` 컬럼이 이미 32바이트이므로 principal id를 그대로 담는다. `moderator` 역할(신고 처리)은 유지한다. 0056은 이 테이블에 **ALTER를 하지 않는다**. `added_by`에는 부트스트랩 시 relay principal id를 넣는다(기존 컬럼 의미 "누가 추가했나"와 일치, FK 없음). 운영자 엔드포인트(§3.3)는 기존 스토어와 `AdminRole`을 호출한다.

**purge 순서와 ON DELETE.** 30일 뒤 `purge_after < now()`인 principal을 삭제하는 작업은 한 트랜잭션으로 다음 순서를 지킨다: (1) 소유 봇 각각에 대해 그 봇의 `access_tokens` → `bots` row → 봇 `principals` row(CASCADE) 삭제, (2) 운영자 row는 raw DELETE가 아니라 `relay_operators::remove(actor=relay_principal)`로 제거(감사 행·advisory lock 유지). 마지막 operator였다면 `LastOperator`를 무시하고 강제 제거하되 warn 로그 + 감사 `auth.operator_roster_emptied`를 남긴다(이후 로그인에서 §3.3 부트스트랩이 다시 성립). `added_by = id`는 relay principal id로 UPDATE, (3) `users`/`relay_members`/`channel_members`의 해당 pubkey row는 오늘의 계정 아카이브 경로(`archived_identities`)로 이동, (4) `principals` row 삭제(identities/devices/sessions/refresh_tokens/access_tokens CASCADE). `bots.owner_principal_id`는 의도적으로 `ON DELETE` 없음 — (1)을 건너뛰면 FK 위반으로 실패해 봇을 고아로 남기지 않는다(Rule 1). 이벤트(`events.pubkey`)는 삭제하지 않는다(오늘과 동일).

`relay` principal row는 SQL seed가 아니라 **relay 기동 코드**가 만든다(B1): `AppState` 초기화에서 `ensure_relay_principal()`이 `PrincipalId::generate()` → `INSERT ... ON CONFLICT DO NOTHING` → `SELECT id WHERE kind='relay'`. 부분 유니크 인덱스 `principals_single_relay`가 다중 인스턴스 동시 기동 경합을 막는다. 따라서 `scripts/reconcile-schema-after-pgschema.sql`에 seed DML을 넣을 필요가 없고(Gotcha 7 비해당), `RELAY_PRIVATE_KEY` 설정도 사라진다.

`username`은 `CITEXT` 대신 `lower()` 함수 인덱스로 유일성을 보장한다(`schema.sql`은 pgcrypto만 켜져 있고 pgschema의 citext 지원이 확인되지 않음).

기존 테이블 변경:

| 테이블 | 변경 | 시점 |
|--------|------|------|
| `events.sig` | `NOT NULL` → nullable (0057), 쓰기 중단(Phase 3), 컬럼 삭제(Phase 4) | 0/3/4 |
| `users` | `display_name`/아바타는 `principals` projection(서버가 프로필 변경 시 모든 커뮤니티 row를 한 트랜잭션으로 갱신). `agent_owner_pubkey`는 서버가 `bots.owner_principal_id`로 채움. `nip05_handle`은 Phase 4 삭제 | 0, 4 |
| `relay_members.pubkey TEXT`, `archived_identities.pubkey TEXT`, `channel_members.pubkey` | 값 의미만 변경(principal id hex/bytes) | - |
| `api_tokens`, `pubkey_allowlist`, `join_policy_acceptances`, NIP-FI 테이블 | 유지 → Phase 4 drop | 4 |

### 2.3 토큰 형식

모든 토큰은 불투명(opaque) 32바이트 CSPRNG + base64url, 접두어로 종류를 구분한다. 서버는 SHA-256 해시만 저장한다. JWT를 쓰지 않는 이유: 폐기 확인 때문에 어차피 DB/Redis 조회가 필요하고, 서명키 관리가 늘며, NIP-FI에서 그 복잡성을 이미 겪었다.

| 접두어 | 종류 | 수명 | 클라이언트 저장 | 갱신 |
|--------|------|------|----------------|------|
| `bzl_` | login code (OIDC 콜백 → 클라이언트 1회 교환) | 60초, 1회 | 메모리 | 없음 |
| `bzs_` | user access | 1h | 메모리(Desktop Rust `AppState`, mobile Riverpod, web JS 메모리) | `POST /auth/refresh` |
| `bzr_` | user refresh | 90일 sliding, 사용마다 rotation | OS keychain(desktop `secret_store.rs`), `flutter_secure_storage`, web은 HttpOnly 쿠키 | 사용 시 교체. 재사용 탐지 → 세션 폐기 |
| `bzb_` | bot access (Desktop 호스팅) | 1h, refresh 없음 | acp 프로세스 메모리(`SecretString`), Desktop 미보관 | `POST /auth/token/exchange` (유효한 동안만) + 같은 WS에서 재-AUTH. 구 토큰 60초 grace 후 `reason=exchanged` revoke |
| `bzk_` | bot headless | 무기한, 회수·교체 가능 | 봇 운영 환경 `.env` | 교체 = 새 토큰 발급 → 구 토큰 회수(두 단계) |

**live 토큰 불변식**: bot 하나당 live `bzb_`는 평상시 1개, exchange 직후 60초 동안만 ≤2개, 이후 다시 1개. 이 불변식은 exchange 트랜잭션(새 토큰 insert + 같은 bot의 "grace 중이 아닌" 다른 `bzb_` revoke)과 60초 뒤 grace 만료 태스크가 함께 보장한다. `POST /auth/bots/{id}/token`(Desktop 재발급)은 grace 여부와 무관하게 기존 `bzb_` 전부를 `reason=reissued`로 revoke한다.

### 2.4 폐기 매트릭스

| 행동 | 서버 효과 | WS close 범위 |
|------|-----------|---------------|
| 로그아웃(현재 디바이스) | `sessions.revoked_at`, 그 세션 `access_tokens` revoke, `devices.revoked_at`, 그 디바이스가 host인 `bots`의 `bzb_` revoke(`device_revoked`) | 세션 연결 + host 봇 연결 |
| 다른 디바이스 원격 로그아웃 | 위와 동일(대상 디바이스) | 동일 |
| **모든 다른 기기 로그아웃** (`POST /auth/sessions/revoke-others`) | 현재 세션 제외 모든 **사람** 세션 revoke. **봇 토큰은 건드리지 않는다**(확정 Q9, Telegram 동작) | 다른 디바이스 연결 |
| **모든 봇 토큰 회수** (`POST /auth/bots/revoke-all`) | 소유 봇의 `bzb_`+`bzk_` 전부 revoke(`revoke_all`). 봇 자체는 유지 | 봇 연결 |
| 봇 Stop (Desktop) | 해당 bot `bzb_` revoke(`stopped`) | 그 봇 |
| 봇 삭제 | `bots.deleted_at`, 그 bot의 모든 토큰 revoke(`bot_deleted`), `principals.disabled_at`. 이벤트 히스토리 유지 | 그 봇 |
| headless 토큰 교체 | 새 `bzk_` 발급 → 클라이언트가 구 토큰 회수 | 구 토큰 연결 |
| exchange | 새 `bzb_` insert, 구 토큰 60초 뒤 `exchanged` revoke | **없음** (연결은 재-AUTH로 새 토큰에 재바인딩됨, §3.3) |
| refresh 재사용 탐지 | 해당 `sessions` revoke(`refresh_reused`) + 사용자 알림 이벤트 | 그 세션 |
| 계정 삭제 (`DELETE /auth/account`) | `principals.disabled_at`, `purge_after = now()+30d`, 모든 세션·봇 토큰 revoke(`account_disabled`) | 전부 |
| 운영자 계정 비활성화 | `principals.disabled_at`, 모든 토큰 revoke | 전부 |

WS close 전파: 토큰 revoke 트랜잭션 커밋 후 Redis `buzz:auth:revoked` 채널에 `{token_hash?, session_id?, bot_id?, principal_id?, reason}` publish. 각 relay 인스턴스는 자기 연결 중 **현재 바인딩된** `token_hash`/`session_id`/`bot_id`가 일치하는 것을 닫는다. `reason=exchanged`는 publish하지 않는다(해당 token_hash에 바인딩된 연결은 이미 재-AUTH로 떠났고, 떠나지 않았다면 60초 뒤 자체 만료 타이머가 닫는다).

### 2.5 계정 보안 이벤트 (비밀번호 없는 모델, 확정 Q9)

비밀번호가 없으므로 "비밀번호 변경"은 존재하지 않는다. 사용자가 할 수 있는 계정 보안 행동은 다음 넷이다.

| 행동 | 의미 | 범위 |
|------|------|------|
| 모든 다른 기기 로그아웃 | Telegram "Terminate all other sessions" | 사람 세션만 |
| 모든 봇 토큰 회수 | 봇이 전부 즉시 끊김. Desktop 봇은 다음 시작 때 자동 재발급, headless는 수동 재발급 | 봇 토큰만 |
| 디바이스 개별 로그아웃 | §2.4 | 그 디바이스 + host 봇 |
| 계정 삭제 | 즉시 비활성화, 30일 후 purge(`purge_after`). 30일 내 같은 Google 계정으로 로그인하면 복구 | 전부 |

Google 계정 "연결 해제"는 제공하지 않는다. Google이 유일한 신원이므로 해제 = 계정 삭제다. Apple 등 두 번째 provider가 추가된 뒤에만 "연결된 로그인 수단 관리"가 의미를 갖는다(`identities` row 2개 이상일 때 1개 삭제 허용).
Google 측 세션 상태(비밀번호 변경, 2FA)는 **재확인하지 않는다**. Telegram/Kakao도 외부 IdP 상태를 매 refresh마다 묻지 않는다. refresh가 90일 미사용으로 만료되거나 사용자가 로그아웃하면 다시 Google 로그인이다.

---

## 3. API / 프로토콜

### 3.1 HTTP가 맞는 이유

AGENTS.md는 새 기능을 Nostr 이벤트로 모델링하라고 하지만, 인증은 이벤트가 될 수 없다(이벤트를 보내려면 인증이 먼저). 로그인/토큰/디바이스/봇 등록은 health·NIP-11과 같은 HTTP 전용 표면으로 분류한다. OIDC는 브라우저 리다이렉트를 요구하므로 어차피 HTTP다.

### 3.2 Google OIDC 로그인 플로우 (확정 Q1)

서버가 OIDC Relying Party다. Google client secret은 서버에만 있다. 클라이언트는 PKCE로 "시작한 자"와 "완료하는 자"가 같음을 증명한다.

```
C: code_verifier(43~128자 랜덤), code_challenge = b64url(sha256(verifier)), state 생성
C → 브라우저: GET https://<buzz>/auth/oidc/google/start
        ?state=<client state>&code_challenge=...&client=desktop|mobile|web|cli&redirect_uri=<아래 표>&device_name=...
S: 클라이언트 state를 그대로 Google `state`로 쓴다. Redis `auth:oidc:pending:{state}` (10분 TTL, 1회)에
   {code_challenge, client, redirect_uri, device} 저장. 이미 존재하는 state → 400
S → 302 → accounts.google.com/o/oauth2/v2/auth?client_id=<server>&redirect_uri=https://<buzz>/auth/oidc/google/callback
        &response_type=code&scope=openid email profile&state=<state>&nonce=...
(사용자 Google 로그인)
Google → S: GET /auth/oidc/google/callback?code=...&state=<state>
S: pending 조회·소비(없으면 400). Google 토큰 엔드포인트에 code 교환(client_secret), id_token 검증(JWKS, iss, aud, nonce, exp)
S: identities(google, sub) 조회 → 없으면 principals+identities 생성(이름/아바타 = Google 프로필 초기값). 한 트랜잭션.
   동시 첫 로그인으로 (provider, subject) PK 충돌 시 → 롤백 후 재조회로 수렴(생성된 principal을 쓰고 자신의 임시 principal은 버림)
S: 운영자 부트스트랩 검사(§3.3 B8)
S: login_code `bzl_` (60초, 1회) 생성, Redis에 {principal_id, code_challenge, device} 저장
S → 302 → <redirect_uri>?code=bzl_...&state=<state>
C: 받은 state == 자기가 보낸 state 확인(불일치 → 로그인 실패, 코드 폐기). POST /auth/oidc/complete {login_code, code_verifier}
S: sha256(code_verifier) == code_challenge 확인, login_code 소비, devices+sessions+refresh_tokens+access_tokens insert (한 트랜잭션)
S → C: {principal_id, access: bzs_, refresh: bzr_, expires_in} (web은 refresh를 Set-Cookie로)
```

| 클라이언트 | 브라우저 | `redirect_uri` | 비고 |
|------------|----------|----------------|------|
| Desktop (Tauri) | 시스템 브라우저(`tauri-plugin-opener`) | `http://127.0.0.1:<임시포트>/cb` (loopback, RFC 8252 §7.3) | Rust가 로그인 동안만 loopback 리스너를 띄운다. 응답은 "Buzz로 돌아가세요" 정적 HTML. 5분 타임아웃 |
| Mobile (Flutter) | `flutter_web_auth_2`(iOS `ASWebAuthenticationSession`, Android Custom Tabs) | `xyz.block.buzz://auth/cb` (custom scheme) | PKCE 동일 |
| CLI (`buzz auth login`) | 시스템 브라우저 | `http://127.0.0.1:<임시포트>/cb` | Desktop과 동일 loopback. `devices.platform='cli'` |
| Web (`web/`) | 같은 탭 리다이렉트 | `https://<relay-origin>/auth/cb` | 완료 후 refresh는 `Set-Cookie: buzz_refresh=...; HttpOnly; Secure; SameSite=Strict; Path=/auth/refresh`, access는 JS 메모리 |

Web 쿠키 제약: `web/`은 relay가 직접 서빙하므로(`router.rs:406 ServeDir`) same-origin이고 `SameSite=Strict`가 성립한다. web을 별도 origin에 두려면 `SameSite=None` + CORS 허용 목록 + CSRF 토큰이 추가로 필요하며 이 계획 범위 밖이다. web의 `/auth/refresh` 응답 본문은 refresh를 **생략**하고 `Set-Cookie`로만 회전시킨다. `POST /auth/logout`은 `Set-Cookie: buzz_refresh=; Max-Age=0`로 쿠키를 지운다.

서버가 허용하는 `redirect_uri`: `http://127.0.0.1:*/cb`(desktop, cli), 설정된 custom scheme(mobile), relay origin(web). 그 외는 400. `AUTH_TOKEN_ENABLED=false`(Phase 0 기본)이면 `/auth/*` 전체(OIDC 포함)가 라우팅되지 않아 미지 경로와 똑같이 응답하고(현재 fallback 403) WS 토큰 AUTH도 거절된다.
provider 확장: `AUTH_OIDC_PROVIDERS=google[,apple]` 설정과 `identities.provider` CHECK 확장만으로 Apple을 추가한다. 경로는 `/auth/oidc/{provider}/start|callback`으로 provider가 경로 변수다.

### 3.3 엔드포인트 (`/auth/*`)

| Method/Path | 인증 | 요청 | 응답 | 비고 |
|-------------|------|------|------|------|
| `GET /auth/oidc/{provider}/start` | 없음 | 쿼리(§3.2) | 302 → IdP | state/pending 10분 |
| `GET /auth/oidc/{provider}/callback` | 없음 | IdP 쿼리 | 302 → `redirect_uri?code=bzl_` | 가입 자동 |
| `POST /auth/oidc/complete` | 없음 | `{login_code, code_verifier}` | `{principal_id, access, refresh, expires_in}` | PKCE 검증. 한 트랜잭션 |
| `POST /auth/refresh` | 없음(본문 또는 쿠키의 refresh) | `{refresh}` | `{access, refresh(new), expires_in}` | rotation(§3.6). 재사용 → 세션 revoke + 401 `refresh_reused` |
| `POST /auth/logout` | Bearer user | `{}` | 204 | 매트릭스 "로그아웃" |
| `GET /auth/devices` | Bearer user | - | `[{id,name,platform,last_seen_at,current}]` | |
| `DELETE /auth/devices/{id}` | Bearer user | - | 204 | 원격 로그아웃 |
| `POST /auth/sessions/revoke-others` | Bearer user | `{}` | 204 | 사람 세션만 |
| `GET /auth/me` | Bearer any | - | `{principal_id, kind, display_name, avatar_url, username, device_id?, bot?:{owner,host}, operator:bool}` | |
| `PATCH /auth/profile` | Bearer user | `{display_name?, avatar_url?, username?}` | 200 | 전역 프로필. `users` projection + 각 커뮤니티 kind 0 재발행(§3.7) |
| `DELETE /auth/account` | Bearer user | `{}` | 204 | 30일 purge 예약 |
| `POST /auth/bots` | Bearer user | `{display_name, host:"this_device" or "headless"}` | `{bot_id}` | principals(kind=bot)+bots 한 트랜잭션 |
| `DELETE /auth/bots/{id}` | Bearer user(owner) | - | 204 | |
| `PATCH /auth/bots/{id}/profile` | Bearer user(owner) | `{display_name?, avatar_url?}` | 200 | 확정 Q5: owner 세션이 봇 프로필 편집 |
| `POST /auth/bots/{id}/token` | Bearer user(owner, host 디바이스 세션) | `{}` | `{token: bzb_, expires_at}` | 기존 `bzb_` 전부 `reissued` revoke |
| `POST /auth/bots/{id}/revoke` | Bearer user(owner) | `{}` | 204 | Stop |
| `POST /auth/bots/revoke-all` | Bearer user | `{}` | 204 | 소유 봇 토큰 전부 |
| `POST /auth/token/exchange` | Bearer bot(`bzb_`) | `{}` | `{token: bzb_(new), expires_at}` | 순서: (1) Redis `auth:exchange:replay:{old_hash}`(10초) 있으면 같은 새 토큰 재전송, (2) `superseded_at` 설정됨 → 409 `token_superseded`, (3) 정상 exchange 후 캐시 기록. 구 토큰 60초 grace. 2회/분/봇 |
| `POST /auth/bots/{id}/headless-token` | Bearer user(owner) | `{}` | `{token: bzk_}` | 1회 표시 |
| `DELETE /auth/bots/{id}/headless-token/{hash_prefix}` | Bearer user(owner) | - | 204 | 교체 2단계 |
| `GET /auth/operators`, `PUT /auth/operators/{principal}` `{role}`, `DELETE /auth/operators/{principal}` | Bearer operator(`AdminRole::Operator`) | | | 확정 Q10. 기존 `relay_operators` 스토어 + `AdminRole` 재사용(B5). `role ∈ operator\|moderator` |

**운영자 부트스트랩 (B8).** 설정 `RELAY_OPERATOR_BOOTSTRAP_EMAIL`. 부여 조건은 "첫 로그인"이 아니라 **매 OIDC 로그인마다** 평가한다: `id_token.email_verified == true` AND `email`이 설정값과 일치(대소문자 무시) AND `relay_operators`에 `role='operator'` row가 **0개** → 그 principal을 `operator`로 1회 insert(`added_by` = relay principal) + 감사 `auth.operator_bootstrapped`. 운영자가 하나라도 있으면 설정은 무시된다(이미 로그인한 사용자도 설정을 뒤에 추가하면 다음 로그인에서 부여되므로 잠금 없음, Rule 6). 운영자가 전원 삭제되면 다시 부트스트랩이 가능하다. 부트스트랩의 "0개" 판정은 **DB row만** 본다 — config 운영자(`RELAY_OPERATOR_PUBKEYS`)가 있어도 무시한다(config 운영자는 키 기반이라 토큰 세계에서 로그인할 수 없으므로). 반대로 `PUT/DELETE /auth/operators`는 Phase 2까지 기존 `config_operator_exists(config)`(`admin/mod.rs:1435`)를 그대로 통과시켜 `LastOperator` 판정이 오늘과 같게 동작한다. `RELAY_OPERATOR_PUBKEYS`/`RELAY_OWNER_PUBKEY` config 우선순위는 Phase 2에서 제거하고 로스터만 남긴다.

401 본문은 `{"error":..., "code": invalid_token | token_expired | token_revoked | refresh_reused | principal_disabled}`. acp와 CLI는 `token_expired|token_revoked|principal_disabled`를 **터미널 인증 실패**로 분류한다.

### 3.4 WS 인증 (NIP-42 대체)

```
S→C: ["AUTH", "challenge"]            # Phase 0–2: 유지(구 클라이언트). Phase 3: 제거
C→S: ["AUTH", {"token": "bzs_..."}]   # 신규. 첫 프레임
S→C: ["OK", "auth", true, ""]         # 또는 ["OK","auth",false,"auth-required: token_expired"]
```

- `AuthState::Pending → Authenticated(AuthContext)` 전이는 그대로. `AuthContext`에 `principal`, `kind`, `token_hash`, `expires_at`, `device_id`, `session_id`, `bot_id`, `bot_owner`, `is_operator` 추가.
- **재-AUTH (B3 해결)**: `Authenticated` 상태에서 다시 `["AUTH", {token}]`이 오면 같은 principal일 때만 수락하고, 연결의 **바인딩을 새 토큰으로 교체**한다: `token_hash`, `expires_at`, 만료 타이머 리셋. 구독(`REQ`)과 진행 중 턴은 유지된다. 다른 principal이면 `OK false "auth-required: principal mismatch"` 후 종료. acp는 exchange 성공 직후 같은 연결에서 재-AUTH하므로 구 토큰이 60초 뒤 revoke되어도 연결은 영향받지 않는다.
- 연결 만료 타이머: `expires_at`에 `CLOSED`+`NOTICE auth-expired`+close. `nip_fi_gate::SessionAdmissionGate`의 "만료 후 효과 금지"를 `SessionGate`로 일반화해 재사용.
- Revoke 전파는 §2.4의 바인딩 키로 매칭한다.
- 핸드셰이크 헤더 토큰은 브라우저 `web/`이 못 쓰므로 채택하지 않는다. 오디오 WS(`audio/handler.rs`)도 같은 AUTH 프레임 형식을 쓴다(§4.4).

### 3.5 서버 측 sender 스탬프와 이벤트 JSON 모양 (결정)

**결정: Nostr 형태를 유지하고 `sig`만 제거한다. `pubkey` 필드명은 유지하며 값은 principal id(hex 64, 유효 x-only 공개키)다.**

| 선택지 | 판정 |
|--------|------|
| A. `pubkey` → `sender` 리네임 | 기각. DB 컬럼·인덱스·FK, `authors` 필터, `p` 태그, desktop/mobile 파서까지 수백 지점의 순수 리네임이며 의미 이득이 없다. 의미는 `PrincipalId` newtype과 문서로 표현한다. |
| B. 필드명 유지, 값 = principal id, `sig` 제거 | **채택**. 와이어·스키마·필터·클라이언트 파서 무변경. §2.1 불변식이 전제. |
| C. 완전 새 포맷 | 기각. |

클라이언트가 보내는 것(EVENT 프레임, `POST /events`)은 draft다:

```json
{"kind": 40002, "created_at": 1760000000, "tags": [["h","<channel-uuid>"]], "content": "hi",
 "pubkey": "<optional; 있으면 principal과 일치해야 함>", "id": "<optional; 서버가 재계산>"}
```

서버 처리(`handle_event`, `ingest_event` 진입 전):
1. `pubkey`가 있으면 `AuthContext.principal`과 비교, 불일치 → 기존 메시지 `"invalid: event pubkey does not match authenticated identity"` 유지. 없으면 스탬프. 예외 없음(봇 프로필은 `PATCH /auth/bots/{id}/profile`로만 변경되고 kind 0은 서버 발행 전용이므로 owner→bot 예외가 필요 없다, §3.7).
2. `created_at` 없음 또는 서버 시각과 ±5분 초과 → 서버 시각으로 교정.
3. `id` = NIP-01 직렬화 SHA-256을 서버가 항상 재계산. `verify_id()`와 `events.id` 유니크 제약 그대로.
4. `sig`: 와이어에서 무시. 내부 `nostr::Event`는 Phase 0–3 동안 64바이트 0 sentinel로 구성. 직렬화 경계(`protocol.rs`, `api/events.rs`)에서 `sig` 제외. Phase 4에서 `buzz_core::event::BuzzEvent`(sig 없음)로 타입 교체. 교체 전까지 §2.1 불변식(유효 x-only 공개키)이 `nostr::Event` 구성의 전제조건이다.
5. 릴레이 발행 이벤트(워크플로우 sink, 40099 시스템 메시지, side-effect, `bridge.rs:799,2628` synthesize_presence, `audio/handler.rs:2984,3429`)는 `relay` principal로 스탬프. `state.relay_keypair` → `state.relay_principal`.

`OK`는 서버가 계산한 최종 `id`를 돌려준다. CLI의 `{event_id, accepted, message}` 계약은 그대로.

### 3.6 Refresh rotation과 재사용 탐지 (B2 해결)

```
POST /auth/refresh {refresh=R_n}
  h = sha256(R_n)
  Redis GET auth:refresh:replay:{h} 있음 (10초 TTL)  → 캐시된 {access, refresh(new)} 재전송 → 200 (세션 변화 없음)
  row = refresh_tokens WHERE token_hash=h
  없음                      → 401 invalid_token  (세션 변화 없음)
  row.used_at IS NOT NULL   → sessions.revoked_at=now(), reason='refresh_reused'
                              + 그 세션 access_tokens revoke + Redis publish → 401 refresh_reused
  row.expires_at < now()    → 401 token_expired
  sessions.revoked_at 있음  → 401 token_revoked
  정상: 한 트랜잭션으로
     UPDATE refresh_tokens SET used_at=now() WHERE token_hash=h
     INSERT refresh_tokens (new hash, generation=row.generation+1, expires_at=now()+90d)
     UPDATE sessions SET last_refreshed_at=now()
     INSERT access_tokens (bzs_, 1h)
     기존 그 세션의 bzs_는 유지(1h 내 자연 만료)
  커밋 후 Redis SET auth:refresh:replay:{h} = {access, refresh(new)} EX 10
  → 200
```

구 row를 `used_at`으로 남기므로 재사용이 탐지된다(v1의 단일 컬럼 덮어쓰기는 불가능했다). 세션당 live refresh 1개는 `refresh_tokens_live` 부분 유니크 인덱스가 보장한다. 네트워크 재시도로 같은 R_n이 두 번 도착하는 정상 케이스는 **첫 단계의 10초 replay 캐시**가 흡수한다(캐시 조회가 `used_at` 분기보다 먼저). 10초를 넘긴 재사용만 공격으로 간주한다. 이 캐시는 새 refresh **평문**을 Redis에 10초 보관한다는 비용이 있다(§6.5).

### 3.7 전역 프로필과 kind 0 (확정 Q2)

- source of truth = `principals.display_name/avatar_url/username`. `PATCH /auth/profile`로만 변경.
- 서버가 변경 트랜잭션 안에서 그 principal의 모든 `users` row(커뮤니티별)를 갱신하고, 커밋 후 각 커뮤니티에 kind 0 이벤트를 principal 스탬프로 발행한다(클라이언트가 이미 kind 0을 구독·캐시하므로 과도기 호환). 클라이언트가 직접 올린 kind 0은 Phase 1부터 거절(`"blocked: profile is managed via /auth/profile"`).
- 봇 프로필도 동일: `PATCH /auth/bots/{id}/profile`(owner). desktop `sync_managed_agent_profile`은 이 엔드포인트를 호출한다.
- Phase 4: kind 0 fan-out을 제거하고 `GET /auth/profiles?ids=`로 대체할지는 Phase 2 이후 성능 데이터로 결정(이 문서 범위 밖).

### 3.8 태그와 kind의 운명

| 항목 | 결정 |
|------|------|
| `h`, `p`, `e`, `d`, `t`, `imeta` 등 | 유지. `p` 값은 principal id. |
| `auth` (NIP-OA) | 제거. |
| kind 22242, 27235, 24242 | 제거. |
| kind 1059 (gift wrap) | 제거(확정 Q4, B7). **DM 모델은 무변경**: 오늘처럼 `h` 스코프 비공개 채널의 kind 40002 + kind 30622 가시성. 40002는 `requires_h_channel_scope`(ingest.rs:769)에 남고 새 p-gate는 만들지 않는다. 바뀌는 것은 "클라이언트가 1059로 감싸던 암호화 레이어"만이며, 기존 1059 이벤트는 Phase 3에서 복호화 불가 데이터로 `deleted_at` 처리. |
| kind 0 | 유지, 서버 발행 전용(§3.7). |
| 나머지 kind, `authors` 필터 | 유지. |

---

## 4. 컴포넌트별 변경 목록

### 4.1 `buzz-core`
- `src/principal.rs`(신규): `PrincipalId` newtype — `generate()`(§2.1), `TryFrom<[u8;32]>`/`from_hex`(x-only 검증), `as_public_key()` 어댑터(전환기). `PrincipalKind`.
- `src/event.rs`: `StoredEvent.verified` 의미 재문서화. Phase 4: `BuzzEvent`.
- `src/verification.rs`: 서명 검증 호출 제거(Phase 3), `verify_id` 유지.
- `src/kind.rs`: `KIND_AUTH`, 27235, 24242, 1059 상수 삭제(Phase 4).

### 4.2 `buzz-auth`
- 신규 `token.rs`: `generate_token(TokenKind) -> (SecretString, [u8;32])`, `parse_prefix`, 상수 시간 비교.
- 신규 `oidc.rs`: provider 레지스트리(`GoogleProvider` 구현, `OidcProvider` trait), discovery/JWKS 캐시, id_token 검증(iss/aud/nonce/exp), PKCE 검증. NIP-FI의 JWKS 캐시 코드(`nip_fi/jwks.rs` 상당)를 이쪽으로 옮겨 재사용 후 nip_fi 삭제.
- `lib.rs`: `AuthContext { principal, kind, token_hash, scopes, channel_ids, auth_method: AuthMethod::Token, expires_at, device_id, session_id, bot_id, bot_owner, is_operator }`. `pubkey`/`agent_owner_pubkey`는 Phase 0–2 유지(래핑), Phase 3 삭제.
- `AuthService::verify_token(hash) -> AuthContext` (trait `TokenStore`로 DB 추상화).
- 삭제(Phase 3/4): `nip42.rs`, `nip98.rs`, `nip98_replay.rs`, `nip_fi/`.
- `rate_limit.rs`: trait는 유지. 프로덕션 구현은 **기존 `buzz-pubsub/src/rate_limiter.rs::RedisRateLimiter`**를 사용한다(v1의 "없다"는 오기). 로그인/exchange/토큰 발급 한도 키를 추가.

### 4.3 `buzz-db`
- `store/principal.rs`, `store/identity.rs`, `store/session.rs`(sessions+refresh_tokens), `store/bot.rs`, `store/access_token.rs`(신규). 운영자는 기존 `store/relay_operators.rs` 재사용(B5). 쓰기는 **반드시 기존 함수 경유**: 부트스트랩·`PUT /auth/operators`는 `upsert(pubkey, role, actor=relay_principal 또는 호출자, cfg)`, `DELETE`는 `remove()` — 그래야 `relay_operator_audit` 행과 대상별 advisory lock이 유지된다. 추가하는 것은 `bootstrap_operator(principal)` 하나이며 내부는 한 트랜잭션에서 `acquire_roster_lock()` → operator row count → 0이면 `upsert(..., "operator", relay_principal, cfg)`. `DbError::LastOperator` → HTTP 409. 각 사용자 행동은 단일 트랜잭션(Rule 5). purge 작업 `purge_expired_principals()`는 §2.2 순서.
- `store/user.rs`: `ensure_user_for_authorization`가 bot이면 `agent_owner_pubkey`를 채우고, `display_name`은 `principals`에서 복사.
- `store/event.rs:881-914 row_to_stored_event`: 변경 없음(불변식 덕분). 단, 테스트에 "`PrincipalId::generate()` 1000개가 모두 `from_slice` 통과"를 추가해 B1 불변식을 production seam에 고정.
- 삭제(Phase 4): `store/api_token.rs`, `store/allowlist.rs`.
- 마이그레이션 0056(§2.2), 0057(`events.sig` nullable), 0058(`_operator_global_tables` 등록), Phase 4 0060대(drop). `schema/schema.sql` 동시 갱신.

### 4.4 `buzz-relay`
- `api/auth.rs`(신규): §3.3 핸들러. `api/oidc.rs`(신규): §3.2. `api/bearer.rs`: Bearer 추출기. `router.rs` 마운트.
- `api/bridge.rs:98-176 verify_bridge_auth_with_options`: `Bearer` 분기 추가(Phase 0), `Nostr`/`X-Pubkey` 삭제(Phase 3). 반환 `VerifiedBridgeAuth.pubkey = principal`. `:799, :2565-2628 synthesize_presence`의 `relay_keypair` 서명 → `relay_principal` 스탬프(Phase 2). `:1426` 호출자 동반 수정.
- `api/admin/auth.rs:371-434 authorize_nip98` 옆에 `authorize_bearer` 분기 추가(**Phase 1**, B6: Desktop 관리 콘솔 `commands/admin/client.rs`가 Phase 1부터 키가 없다): `access_tokens` 조회 → principal → 기존 `resolve_admin_principal`/`AdminRole`(DB 로스터, 확정 Q10·B5). 진입점 `authorize()`(`:197-246`)는 `AdminAuth` enum으로 모드를 고르므로, Phase 1–2 공존을 위해 `Authorization: Bearer bz…` 헤더는 **`Nip98` 모드에서도** `authorize_bearer`로 분기한다(또는 `AdminAuth::Token` variant를 추가하고 `Nip98` 모드가 Bearer를 위임). 모드 분기 바깥에서 헤더 스킴으로 먼저 가른다. NIP-98 분기와 `nostr_credential()`(459)는 Phase 3 삭제. 브라우저 origin 검사(467-)는 유지. config 우선순위(`RELAY_OPERATOR_PUBKEYS`/`RELAY_OWNER_PUBKEY`)는 Phase 2에서 제거.
- `api/git/transport.rs:377 parse_git_auth_header_full`(호출부 `:160-180`)에 `Authorization: Basic base64(token:<token>)` / `Bearer` 분기 추가(**Phase 1**, B6: 호스팅 에이전트의 git push가 Phase 1부터 토큰이어야 한다). git은 Basic을 보내므로 username `token` 고정. 정책 훅 pusher 식별 = principal. NIP-98 분기는 Phase 3 삭제.
- `audio/handler.rs:33,370-400,495-576` NIP-42 챌린지/검증 → §3.4 토큰 AUTH 프레임(Phase 2). `:2984,:3429` `relay_keypair` 서명 → `relay_principal` 스탬프. `audio/mesh.rs` 삭제(Phase 4).
- `invite_token.rs:111 derive_invite_key(&Keys)` → `INVITE_SIGNING_SECRET`(32바이트 hex, 필수 설정)에서 HKDF로 파생(Phase 2). 설정 없으면 기동 실패. 기존 발행 초대는 무효화되므로 Phase 2 배포 노트에 명시.
- `handlers/auth.rs`: 토큰 객체 분기 + 재-AUTH 바인딩 교체(§3.4). NIP-42 분기 Phase 3 삭제. ban/relay_membership 체크 그대로.
- `handlers/event.rs:629-660`: draft 스탬프, kind 0 직접 발행 차단, gift wrap 예외 삭제(Phase 3). `requires_h_channel_scope`(ingest.rs:769)는 그대로(B7).
- `handlers/ingest.rs:2402-2420`: 서명 검증 → id 재계산.
- `connection.rs`: `AuthState::Authenticated`에 바인딩 필드, 만료 타이머, 재-AUTH. `ConnectionManager::close_by_binding(BindingKey)`.
- `state.rs`: `relay_keypair` → `relay_principal`(기동 시 `ensure_relay_principal`). Redis `buzz:auth:revoked` 구독. OIDC provider 레지스트리.
- `config.rs`: 제거 — `relay_owner_pubkey`, `relay_operator_pubkeys`(Phase 2, 로스터만 남김); `relay_private_key`, `allow_nip_oa_auth`, `pubkey_allowlist_enabled`, `require_auth_token`(Phase 3/4). 추가 — `auth.access_ttl/refresh_ttl/bot_ttl`, `auth.oidc.google.{client_id,client_secret}`, `auth.redirect_allowlist`, `invite_signing_secret`, `relay_operator_bootstrap_email`(§3.3 B8 조건).
- `nip11.rs`: 42/98 제거, `auth: "bearer"`.
- 삭제: `nip_fi_*.rs`, `api/nip_fi.rs`, `nip98.rs`, `api/mesh_demo.rs`.

### 4.5 `buzz-pubsub`
- `presence.rs`: 키 `buzz:{community}:presence:{pubkey_hex}` 그대로. 타입 `&PrincipalId`.
- `rate_limiter.rs`: 기존 `RedisRateLimiter` 재사용. 키 네임스페이스 `auth:login:{ip}`, `auth:exchange:{bot}`, `auth:bot_token:{user}`.
- 신규 `auth_revocation.rs`: `buzz:auth:revoked` publish/subscribe. `nip_fi_command_replay.rs`, `nip98_replay.rs` 삭제(Phase 3).

### 4.6 `buzz-audit`
- `entry.rs actor_pubkey` 바이트 의미만 변경. TLV 포맷 불변. 신규 액션 `auth.login`, `auth.logout`, `auth.device_revoked`, `auth.sessions_revoke_others`, `auth.bots_revoke_all`, `auth.bot_created/deleted`, `auth.token_exchanged`, `auth.refresh_reuse_detected`, `auth.account_deleted`, `auth.operator_granted/revoked`.

### 4.7 `buzz-media`
- `auth.rs verify_blossom_*` → Bearer principal(Phase 2). 업로드 소유자 = principal.

### 4.8 git 크레이트
- `git-credential-nostr` → `git-credential-buzz`(**Phase 1**, B6): 키 로딩 삭제. 토큰 소스 `BUZZ_BOT_TOKEN` → `BUZZ_ACCESS_TOKEN` → `BUZZ_TOKEN_BROKER_URL`+`_SECRET` → `git config buzz.tokenfile`. 출력 `username=token`, `password=<token>`. `buzz-acp/src/git.rs:99-100`이 자식 env에 `BUZZ_PRIVATE_KEY`를 넣던 자리는 브로커 변수로 교체.
- `git-sign-nostr`: 삭제(Phase 4). `buzz-acp/src/git.rs` 서명 설정 주입 제거.

### 4.9 `buzz-ws-client`
- `connection.rs`: `connect_with_token(url, &SecretString)`, `reauth(&SecretString)`(같은 연결 재-AUTH, `OK auth` 대기), `publish_draft`. `message.rs::build_auth_event` 삭제.

### 4.10 `buzz-sdk`
- `builders.rs`: `EventBuilder` → `into_draft()`. `nip_oa.rs` 삭제.

### 4.11 `buzz-acp`
- `config.rs:251-256`: `--bot-token/BUZZ_BOT_TOKEN`(`hide_env_values`). `BUZZ_ACP_PRIVATE_KEY` alias(1028) 삭제. `Keys::parse` 삭제.
- 토큰 보관: `SecretString`. 자식(에이전트) 스폰 시 **`Command::env_remove("BUZZ_BOT_TOKEN")`**로 상속 차단. 프로세스 전역 `std::env::remove_var`를 쓰지 않는 이유: 워크스페이스는 edition 2021이라 `unsafe`는 아니지만, acp 자신이 토큰을 다시 읽어야 하는 재연결·exchange 경로와 경합하고 다른 스레드의 env 읽기와 데이터 레이스 위험이 있다. 스폰 단위 제거가 정확하다.
- `relay.rs:3735-3762` AUTH → 토큰 프레임. `sign_with_keys`(464, 1148) → draft. 재연결 시 현재 토큰.
- 신규 `token_refresh.rs`: 만료 15분 전 exchange → 성공 시 메모리 교체 → **즉시 같은 연결에서 재-AUTH** → `OK auth true` 확인 후 세대 증가(`AtomicU64`; 더 새로운 세대만 채택, Rule 2). 실패 분류: 네트워크/5xx → 기존 `STARTUP_CONNECT_BACKOFFS` 상한 재시도(10초 내 재시도는 서버 replay 캐시가 같은 토큰을 돌려준다, §6.5); HTTP 401 `token_expired|token_revoked|principal_disabled`, **HTTP 409 `token_superseded`**, **또는 WS `OK auth false` 사유가 같은 코드** → exit 78(auth terminal). WS 재연결 시 AUTH 거절도 같은 분류를 거친다(B3 후반).
- 신규 `token_broker.rs`: 127.0.0.1 임시 포트 HTTP `GET /token`, 스폰당 랜덤 secret `Authorization` 요구, 응답 ≤4KB, 분당 60회. 에이전트 env에 `BUZZ_TOKEN_BROKER_URL`, `BUZZ_TOKEN_BROKER_SECRET`만 전달(확정 Q3).
- `pool.rs:5573-5793` `sign_with_keys` → draft. `ctx.agent_keys` → `ctx.principal`.
- `setup_mode.rs`, `run_task.rs`, `tests/stdio_contract.rs` env 계약 갱신.

### 4.12 `buzz-cli`
- **Phase 1 (B6)**: `client.rs:519-591`에 토큰 모드 추가 — 토큰 소스 `BUZZ_BOT_TOKEN` → `BUZZ_ACCESS_TOKEN` → `BUZZ_TOKEN_BROKER_URL`+`_SECRET`(§4.8 우선순위) → 없으면 기존 `BUZZ_PRIVATE_KEY` 키 모드(Phase 3까지 폴백). 토큰 모드에서 `sign_nip98`/`sign_blossom_*`/`sign_event` → Bearer + draft. 호스팅 에이전트 안의 `buzz messages …`가 Phase 1부터 동작해야 한다.
- **Phase 2**: 신규 `commands/auth.rs`: `buzz auth login`(§3.2 loopback 플로우, `client=cli`), `logout`, `whoami`, `devices [revoke]`, `bots {create,delete,token,headless-token,revoke,revoke-all}`; `~/.config/buzz/session.json`(access 캐시; refresh는 keyring)을 토큰 소스 마지막 순위에 추가.
- `error.rs` 종료 코드 계약 불변. 401 terminal → 3.

### 4.13 `buzz-workflow`
- `executor.rs:994-998`: `BUZZ_API_TOKEN`/`BUZZ_RELAY_PUBKEY`→`X-Pubkey` 폴백 → 워크플로우 sink는 `relay_principal` 내부 경로 또는 `BUZZ_BOT_TOKEN`(워크플로우 전용 headless 봇)만 사용. `X-Pubkey` 삭제(Phase 3).
- `executor.rs:194 npub 필터`: `PublicKey::from_hex` 성공 시 bech32 — Phase 4에서 `principal` 필터(hex 앞 8자)로 교체. 그 전까지는 불변식상 동작.

### 4.14 Desktop (Rust, `desktop/src-tauri`)
- `app_state.rs`: `keys: Mutex<Keys>` → `session: Mutex<Option<UserSession{principal, device_id, access: SecretString, access_expires_at}>>`. refresh는 `secret_store.rs`. `signing_keys()` → `access_token()`.
- 신규 `auth/`: `oidc_login.rs`(loopback 리스너 + 시스템 브라우저 + PKCE + `complete`), `refresh_task.rs`(만료 10분 전, 지터, 지수 백오프, 터미널 "재로그인 필요"), `devices.rs`, 명령 `get_ws_auth_frame`, `login_with_google`, `logout`, `list_devices`, `revoke_device`, `revoke_other_sessions`, `revoke_all_bot_tokens`, `delete_account`.
- `commands/identity.rs`: `get_identity` → principal/프로필. `sign_event`(135) → `publish_event(draft)`. `get_nsec`, 백업/import/export, `build_nostr_identity_binding_event`, `nostr_bind.rs` 삭제.
- `relay.rs:120-160` NIP-98 → bearer. `submit_event_with_keys` → `submit_draft`. `sync_managed_agent_profile`(551) → `PATCH /auth/bots/{id}/profile`. `commands/admin/client.rs` 관리 콘솔 호출도 Phase 1에서 Bearer(세션 access)로 전환(B6; 서버 `authorize_bearer`가 같은 Phase).
- `native_relay_client.rs:430-482` → 토큰 + 재-AUTH.
- `managed_agents/types.rs:298-324`: `private_key_nsec`, `auth_tag` 삭제. `pubkey` = bot id 유지. 기존 레코드: keyring nsec 삭제, 서버에 `POST /auth/bots` 새로 등록(히스토리 단절 수용, 확정 Q7). 첫 실행 시 1회 안내.
- `managed_agents/runtime.rs:639` → `POST /auth/bots/{id}/token` 후 `BUZZ_BOT_TOKEN` 주입. `:773-776` `BUZZ_AUTH_TAG` 삭제. `:771` `env_remove` 목록에 `BUZZ_BOT_TOKEN`, `BUZZ_ACCESS_TOKEN`, `BUZZ_TOKEN_BROKER_URL`, `BUZZ_TOKEN_BROKER_SECRET` 추가(사용자 env가 덮지 못하게; 주입은 그 뒤).
- exit 78 처리: 토큰 재발급 → 재시작, **10분 창 3회 상한**. 상한 도달 → 상태 `AuthFailed`로 고정하고 자동 재시작 중단. UI에 사유와 **"다시 시작" 수동 버튼**(카운터 리셋), 세션 자체가 죽은 경우 "다시 로그인" 버튼(Rule 6).
- `managed_agents/reserved_env_keys.rs:28-96`: 제거 `BUZZ_PRIVATE_KEY`, `BUZZ_AUTH_TAG`, `BUZZ_API_TOKEN`, `BUZZ_ACP_PRIVATE_KEY`, `BUZZ_ACP_API_TOKEN`; 추가 **`BUZZ_BOT_TOKEN`, `BUZZ_ACCESS_TOKEN`, `BUZZ_TOKEN_BROKER_URL`, `BUZZ_TOKEN_BROKER_SECRET`**(명시).
- `managed_agents/hermes_profile.rs:280-282 is_buzz_owned_env_key`: `BUZZ_*` 전체 strip이라 새 키도 자동 포함. `hermes_profile_tests.rs`에 `BUZZ_BOT_TOKEN=…` strip 케이스 명시(Rule 3).
- `commands/agents.rs`: 생성 `Keys::generate()` → `POST /auth/bots`; 삭제는 서버 `DELETE` 성공 후 로컬/Hermes 프로필 삭제(Rule 1).
- 삭제(Phase 4): `key_backup.rs`, `hpke_key_backup.rs`, `identity_storage.rs` 키 마이그레이션, `nostr_bind.rs`.
- 로그 마스킹: `commands/agent_discovery/install_report_redaction_tests.rs` 패턴을 따라 `bz[lsrbk]_[A-Za-z0-9_-]{40,}` redaction + 테스트.

### 4.15 Desktop (React, `desktop/src`)
- `features/onboarding`: 키 생성/임포트 → "Google로 로그인" 버튼(`invoke("login_with_google")`), 대기 화면, 실패/타임아웃 복귀.
- `features/settings`: Account(프로필 편집 전역, Devices 목록·원격 로그아웃, "모든 다른 기기 로그아웃", "모든 봇 토큰 회수", 계정 삭제).
- `shared/api/relayClientSession.ts:277-393` `signRelayEvent` → `invoke("get_ws_auth_frame")` + draft. `relayClientShared.ts`의 `signRelayEvent` 삭제. **`@/shared/api/tauri` 경유 호출자 `communityProfile.ts`, `customEmoji.ts`, `invites.ts`, `moderation.ts`도 draft/HTTP로 교체.**
- `relayAuthPolicy.ts`: `token_expired` → Rust refresh 후 재연결.
- `shared/lib/nostrUtils.ts` npub/nsec 삭제; `pubkey.ts` hex 검증만. UI "npub" → 이름/@username.
- `useCommunityInit.ts:65-106 resetCommunityState`: 신규 모듈 캐시(디바이스 목록 등) reset 추가.
- `shared/constants/kinds.ts` 동기.

### 4.16 Mobile (`mobile/lib`)
- **Phase 1 선행 작업**: `shared/relay/nostr_models.dart:103-132 NostrEvent.sig`를 `String?`로(required 해제). Phase 1에서 토큰 연결로 쓰인 sentinel/`sig` 없는 이벤트가 mobile 파서에 도달하기 전에 머지되어야 한다.
- `shared/auth/auth_provider.dart`: `nsec` → `{principalId, accessToken, accessExpiresAt}`; refresh는 `flutter_secure_storage`; 앱 시작 시 refresh 자동 로그인. 로그인 = `flutter_web_auth_2` + PKCE(§3.2).
- `relay_session_auth.dart` → `bearerHeader()`. `relay_session.dart:146-179,500` → 토큰 AUTH + 재-AUTH. `signed_event_relay.dart` → draft.
- `shared/crypto/{nip44,ecdh,nip_oa}.dart`, `features/pairing` 삭제. `features/settings`: Account/Devices.

### 4.17 `web/` (Phase 2 범위, 확정 Q6)
- `shared/lib/nostr-signer.ts:42` 임시 키 생성, `nip98.ts`, `features/repos/git-client.ts:57`, `features/invite/invite-api.ts` → Google 로그인 세션(§3.2 web 행): access는 JS 메모리, refresh는 HttpOnly 쿠키, `Authorization: Bearer`. 리포 브라우저의 git 읽기는 Bearer. 로그인 전 공개 읽기는 오늘과 동일하게 토큰 없이.

### 4.18 문서
- `VISION.md` Identity 재작성, `ARCHITECTURE.md` auth, `AGENTS.md`(Agent CLI env: `BUZZ_BOT_TOKEN`/`BUZZ_TOKEN_BROKER_*`; Gotcha 2 유지), `TESTING.md`, `crates/buzz-cli/TESTING.md`, `docs/nips/NIP-OA.md`·`NIP-FI.md` → `docs/spec/auth.md`, `.env.example`(Google client id/secret, `INVITE_SIGNING_SECRET`).

---

## 5. 단계별 계획

모든 Phase는 독립 머지 가능하고 `just ci`/`just test` 녹색. 테스트는 TESTING.md "Review-Proven Test Standards"대로 프로덕션 seam에 바인딩하고, 가드 제거 시 실패해야 한다.

### 5.1 Phase 0 — 서버: principal + 토큰 인증을 키 인증과 병행

> **Phase 0 구현 주의 (리뷰어 top-3 리스크)**
> (a) **sentinel/NULL sig 이벤트의 읽기 경로.** 토큰 연결이 스탬프한 이벤트는 `row_to_stored_event`와 모든 `nostr::Event` 파싱 지점을 통과해야 한다. 플래그 on 상태의 라이브 테스트로 REQ, `POST /query`, push 알림, 워크플로우 sink가 그 이벤트를 `verified=true`로 돌려주는지 확인한다. 하나라도 `Ok(None)`/skip이면 메시지가 조용히 사라진다.
> (b) **`connection.rs` 바인딩 교체.** 재-AUTH 시 구 만료 타이머를 abort하고 새 타이머를 등록한다. `BindingKey`는 **토큰 해시**(session_id/bot_id 아님)여야 교차 인스턴스 `close_by_binding`이 같은 principal의 다른 연결을 닫지 않는다. 10분 DB 재확인은 연결당 태스크 1개로 바운드하고 연결 종료 시 abort.
> (c) **플래그와 스키마.** `AUTH_TOKEN_ENABLED=false`는 `/auth/*`, WS 토큰 AUTH, bridge Bearer 분기, 운영자 부트스트랩을 모두 비활성화해야 한다 — 라우터 레벨 테스트로 고정. 0056–0058은 `schema/schema.sql`과 `_operator_global_tables`(`conformance_multitenant.rs`)를 함께 갱신하고, **pgschema 부트스트랩 경로**에서도 검증한다(AGENTS.md Gotcha 7: 부분 유니크 인덱스·CHECK가 `pgschema apply`로 재현되는지 확인, 안 되면 `reconcile-schema-after-pgschema.sql`에 수렴문+assertion).

범위
- 마이그레이션 0056/0057/0058 + `schema.sql`. `buzz-db` 신규 스토어. `buzz-auth` token/oidc 모듈. `buzz-relay` `/auth/*`(OIDC 포함), Bearer 분기(bridge), WS `["AUTH",{token}]` + 재-AUTH 바인딩 교체, draft 스탬프(토큰 연결만), 만료 타이머, Redis revoke 전파, `ensure_relay_principal`.
- 키 인증 경로 그대로. `AUTH_TOKEN_ENABLED=false` 기본.

테스트(프로덕션 seam)
- `buzz-relay/tests/auth_token_live.rs`(Postgres+Redis): OIDC는 테스트용 가짜 provider(`OidcProvider` trait 구현)로 start→callback→complete 수행 → WS AUTH → draft → OK id → REQ `authors`=principal 확인 → revoke 후 60초 내 close(2 인스턴스 교차).
- **B1**: `PrincipalId::generate()` 1000회 모두 `nostr::PublicKey::from_slice` 통과; `create_principal`에 무효 바이트 → 오류; `row_to_stored_event`가 토큰 스탬프 이벤트를 `Some`으로 반환. relay principal: 두 커넥션이 동시에 `ensure_relay_principal` → 같은 id.
- **B2**: 프로덕션 `POST /auth/refresh` 핸들러를 R1→R2 정상 → R1 재사용(10초 후) → 401 `refresh_reused` **그리고 `sessions.revoked_at IS NOT NULL`** 단언. not-found(`invalid_token`)와 구분. 10초 내 재전송은 같은 R2 반환 + 세션 유지. `used_at` 가드 제거 시 실패.
- **B3**: 연결 A가 토큰 T1으로 AUTH → T1 exchange → 같은 연결에서 T2 재-AUTH → 60초 뒤 T1 revoke → **A는 열려 있음**(REQ 응답 확인). 재-AUTH 없이 60초 경과 → A close. 다른 principal 토큰으로 재-AUTH → 거절+close.
- draft: 다른 pubkey → 거절 메시지 불변(owner가 자기 bot id를 써도 거절); 클라이언트 kind 0 → 차단 메시지.
- OIDC: `/start`에 `state` 없음 → 400; callback의 state가 pending에 없음 → 400; 가짜 provider의 `email_verified=false`로 로그인 → 성공하되 운영자 아님.
- **B8 부트스트랩**: 로스터 비어 있음 + 이메일 일치 + verified → `relay_operators`에 operator 1행 + 감사 로그; 로스터에 operator 1행 이상 → 부여 없음; `email_verified=false` → 부여 없음; 이미 가입한 사용자가 설정 추가 후 재로그인 → 부여됨(잠금 없음). 가드 제거 시 각각 실패.
- DM(B7): `h` 없는 kind 40002 → 기존 `"channel-scoped events must include an h tag"` 거절 유지(회귀 가드).
- rate limit: `RedisRateLimiter`로 exchange 3회째 429.
- 동시 첫 로그인: 같은 `sub`로 두 callback 동시 → principal 1개, identities 1행.
- `conformance_multitenant.rs`: 신규 테이블 `_operator_global_tables` 등록.

수동 검증
```
# dev: AUTH_OIDC_FAKE=1 로 가짜 provider 활성화
curl 'http://localhost:3000/auth/oidc/google/start?code_challenge=...&client=cli&redirect_uri=http://127.0.0.1:9999/cb' -i
curl -X POST http://localhost:3000/auth/oidc/complete -d '{"login_code":"bzl_...","code_verifier":"..."}'
websocat ws://localhost:3000  → ["AUTH",{"token":"bzs_..."}] → ["EVENT",{"kind":40002,...}]
curl -H "Authorization: Bearer bzs_..." -X POST http://localhost:3000/query -d '{"kinds":[40002],"authors":["<principal>"]}'
```

리스크: 구 클라이언트가 sentinel sig 이벤트를 `verified=false`로 볼 수 있음 → 플래그 off 기본, 개발 relay만 on. Google OAuth 클라이언트 등록이 선행 운영 작업: 리다이렉트가 항상 서버의 단일 `/auth/oidc/google/callback`이므로 **"Web application" 타입 1개**만 등록한다(desktop/mobile/cli는 서버 뒤에 있어 별도 클라이언트 불필요).

### 5.1.1 Phase 0 상태 / handoff (2026-10-04)

Phase 0은 구현 후 리뷰 2회를 거쳐 **READY** 판정. 브랜치 `feat/centralized-identity`, **미커밋**(작업 트리에만 존재). `CLAUDE.md`의 `M` 표시는 Windows 심볼릭 링크 아티팩트이므로 stage하지 않는다. 이 절만 읽고 Phase 1을 시작할 수 있도록 정리한다.

**파일 (크레이트별, 신규 = N, 수정 = M)**
- `migrations/`: N `0056_principals.sql`(principals·identities·devices·sessions·refresh_tokens·bots·access_tokens), N `0057_events_sig_nullable.sql`, N `0058_identity_operator_global_tables.sql`(`_operator_global_tables` 등록).
- `schema/schema.sql` M(같은 DDL, `events.sig` nullable); `scripts/reconcile-schema-after-pgschema.sql` M(pgschema가 떨어뜨리는 `access_tokens_kind_shape` CHECK 복원 + 단언); `.env.example` M(`AUTH_*` 변수 블록).
- buzz-core: N `src/principal.rs`(`PrincipalId`, `PrincipalKind`, `AccessTokenKind`); N `src/draft.rs`(`stamp_draft`, `SENTINEL_SIG`, ±300초 보정); M `src/lib.rs`.
- buzz-auth: N `src/token.rs`(접두사 `bzl_/bzs_/bzr_/bzb_/bzk_`, SHA-256 해시, PKCE); N `src/oidc.rs`(`OidcProvider` 트레이트, `GoogleProvider` + JWKS); M `src/lib.rs`(`AuthMethod::Token`, `TokenBinding`, `AuthContext.token`); M `Cargo.toml`(`subtle`, `zeroize`).
- buzz-db: N `src/store/identity/mod.rs`(공용 타입·한도); N `principal.rs`(relay principal, 로그인/가입, 프로필); N `session.rs`(로그인 완료, refresh rotation·재사용 탐지, 디바이스); N `bot.rs`(봇·봇 토큰·exchange); N `access_token.rs`(토큰 조회·검사, 계정 비활성화); N `postgres_tests.rs`; M `src/store/relay_operators.rs`(`bootstrap_operator`); M `src/store/event.rs`, `src/store/reminder.rs`(NULL sig → sentinel 읽기); M `src/runtime/migration.rs`(멀티테넌트 린트 목록, 마이그레이션 58개, 파리티 테스트에 reconcile 실행); M `src/runtime/tests/thread_window_postgres_tests.rs`(버전 58); M `src/lib.rs`, `src/store/mod.rs`.
- buzz-pubsub: N `src/auth_revocation.rs`(`buzz:auth:revoked` 채널, `not_after` 변형); M `src/lib.rs`(publish/subscribe); M `src/rate_limiter.rs`(`check_key`); M `Cargo.toml`(`hex`).
- buzz-relay `identity/`: N `mod.rs`(`IdentityRuntime`, `verify_access_token`, revoke 소비자, 테스트용 `FakeOidcProvider`); N `config.rs`(`AuthTokenConfig::from_env`); N `kv.rs`(Redis pending/login code/replay 캐시); N `ws.rs`(WS 토큰 AUTH, 재-AUTH 바인딩 교체, 바인딩 감시 태스크, draft 스탬프).
- buzz-relay `api/auth/`: N `mod.rs`(라우터, 공용 응답·Bearer·rate limit); N `oidc.rs`; N `session.rs`; N `bots.rs`; N `operators.rs`; N `router_tests.rs`; N `live_tests.rs`.
- buzz-relay 기존 파일 M: `protocol.rs`(`AuthToken`, `EventDraft`, `ParseMode`); `connection.rs`(토큰 AUTH 분기, draft 스탬프, 종료 시 세션 제거); `rejection.rs`(재-AUTH를 `ws_operations` 버킷에 포함); `handlers/ingest.rs`·`handlers/event.rs`(`IngestAuth::Token`, 서버 스탬프 이벤트는 id만 검증); `api/bridge.rs`(`POST /events|/query|/count` Bearer 분기); `router.rs`(플래그 on일 때만 `/auth/*` 마운트); `config.rs`, `state.rs`, `lib.rs`, `main.rs`(기동 시 relay principal + revoke 소비자); `api/admin/mod.rs`(`is_effective_operator`); `api/mod.rs`; `handlers/count.rs`·`req.rs`·`relay_admin.rs`·`api/bridge/thread_window/postgres_tests/failure_postgres_tests.rs`(테스트 `AuthContext`에 `token: None`).

**테스트 실행** (Docker `buzz-postgres` 127.0.0.1:5432, `buzz-redis` 127.0.0.1:6379. `localhost`는 `::1`로 풀려 풀 타임아웃이 나므로 반드시 `127.0.0.1`. Git Bash에서는 `export PATH="$HOME/.cargo/bin:$PATH"` 먼저.)
```
export BUZZ_TEST_DATABASE_URL=postgres://<user>:<pw>@127.0.0.1:5432/buzz   # 자격증명은 crates/buzz-relay/src/test_support.rs 기본값
export BUZZ_TEST_REDIS_URL=redis://127.0.0.1:6379
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p buzz-db identity -- --include-ignored --test-threads=1
cargo test -p buzz-relay --lib api::auth::live_tests -- --ignored --test-threads=1
cargo test --no-fail-fast -p buzz-core -p buzz-auth -p buzz-pubsub -p buzz-db -p buzz-relay
```
- 마지막 결과: live 9/9, buzz-db identity 10/10, 단위 2123 통과 / 6 실패. 6건(observability_source moderation 린트, git `bash_hmac` 2건, `mesh_demo`, `boot_lifecycle` 2건)은 모두 이 브랜치가 수정하지 않은 소스와 테스트 파일에서 나는 환경/기존 실패(boot_lifecycle 등은 HEAD 기준 워크트리에서 재현 확인). `admin_schema_parity_between_desired_state_and_migrations`는 Windows에서 Hermit `bin/pgschema` shim이 실행되지 않아 실패 — Linux 컨테이너(`pgschema-1.7.4-linux-amd64`, `buzz-net`)에서 수동 검증 완료.
- `bash_hmac` 테스트가 `crates/buzz-relay/.concat` 빈 파일을 남길 수 있다. 커밋 전에 삭제.

**CRLF/LF 주의**: 저장소 파일은 LF, 이 머신은 `core.autocrlf=true`. Windows에서 Python 텍스트 모드 쓰기나 원인 불명의 작업 트리 변환으로 CRLF가 들어가면 `include_str!` 기반 소스 린트와 마이그레이션 체크섬이 깨진다. 편집 후 `grep -c $'\r' <file>`로 확인하고 `sed -i 's/\r$//'`로 정규화한다.

**알려진 편차**
- nostr 0.44의 `PublicKey::from_slice`는 곡선 위 점인지 검사하지 않는다(32바이트 0도 통과). `PrincipalId`의 생성자(`TryFrom<[u8;32]>`, `from_slice`, `from_hex`)는 `xonly()`로 곡선 멤버십까지 검증하므로, principal id는 항상 이 타입을 거쳐 만들 것.
- 플래그 off의 `/auth/*`는 리터럴 404가 아니라 "라우팅 안 됨"(미지 경로와 동일, 현재 fallback 403).
- exchange grace는 별도 타이머 없이 구 토큰 `expires_at = LEAST(expires_at, now()+grace)`로 저장하고 revoke 메시지의 `not_after`로 연결 데드라인을 낮춘다.
- refresh는 `RecentlyRotated`, exchange는 `RecentlyExchanged`(DB 시계 기준 10초)로 커밋 경합 재시도를 replay 캐시 폴링(50ms×20)으로 처리.

**후속 (Phase 0에서 하지 않음)**
- 서버 스탬프 여부를 연결 인증 상태가 아니라 메시지 변형(`EventDraft`)에서 도출(TOCTOU 제거).
- 만료 `access_tokens` 행 purge 잡(Phase 1/2), `jwks_for` 최소 재조회 간격.
- 신뢰 프록시 forwarding 헤더(§5.2 선행, §6.6).
- ConnectInfo가 있을 때 refresh / bridge Bearer / `/auth/me`에 **실패 시에만 소비하는** per-IP 버킷(성공 트래픽은 세지 않아 프록시 뒤 붕괴 없이 무차별 대입만 제한).
- 재-AUTH의 DB 일시 오류: 지금은 bind-then-check 실패 시 연결을 닫는다. 기존 바인딩을 유지하고 `OK auth false`만 보내도록 변경(일시 오류와 실제 revoke 구분).
- 캐시 miss 응답 일관성: refresh `RecentlyRotated` 캐시 miss는 503(재시도), exchange `RecentlyExchanged` 캐시 miss는 409(acp exit 78 → 재발급). 의도된 차이(refresh 재시도는 10초 후 재사용 탐지로 세션 revoke 위험, exchange는 재발급이 안전)이나 클라이언트 구현 시 두 코드를 모두 처리할 것.
- 기타: 프로필 변경 시 kind 0 재발행 없음, purge 잡 없음, buzz-audit auth 액션 없음, 닫힌 릴레이에서 봇 멤버십 필요, NIP-11 미갱신, admin/git Bearer 없음(§5.2 B6).

### 5.2 Phase 1 — Desktop 로그인 + acp 봇 토큰

범위
- Desktop Rust `auth/`(§4.14), 로그인 UI, keyring refresh, 백그라운드 refresh, `get_ws_auth_frame`, 봇 등록/토큰/프로필 흐름, `runtime.rs` env 교체, exit 78 재시작(상한+수동 버튼).
- buzz-acp: `BUZZ_BOT_TOKEN`, 토큰 AUTH, draft, `token_refresh.rs`(exchange+재-AUTH), `token_broker.rs`, `Command::env_remove`.
- buzz-ws-client/buzz-sdk 토큰 API 추가(키 API 유지).
- **B6 — 호스팅 에이전트가 Phase 1에서 계속 동작하기 위한 선행**: (a) buzz-cli 토큰 모드(§4.12: `BUZZ_BOT_TOKEN` → 브로커 소스, Bearer + draft, 키 모드 폴백 유지); (b) `git-credential-buzz`(§4.8) + acp `git.rs:99-100` 브로커 변수 주입; (c) 서버 `authorize_bearer`(admin, §4.4)와 git `transport.rs` Bearer/Basic 분기, Desktop `commands/admin/client.rs` Bearer 전환.
- **Mobile 선행 1건**: `NostrEvent.sig` optional(§4.16).
- 기존 managed agent → 새 bot id 재등록 + 1회 안내.
- **선행(배포 게이트)**: 신뢰 프록시 forwarding 헤더 지원(`X-Forwarded-For` / `Forwarded` 또는 PROXY protocol, 신뢰 CIDR 설정) — TCP 리버스 프록시 뒤에서 `AUTH_TOKEN_ENABLED`를 켜기 전에 반드시 들어가야 한다. 그 전에는 모든 클라이언트가 프록시 IP 하나의 버킷을 공유해 `/auth/oidc/complete`가 배포 전체 10/분으로 붕괴한다(§6.6).

테스트
- desktop: `reserved_env_keys` 신규 키 4개가 사용자 env를 덮지 못함; `hermes_profile_tests.rs` `BUZZ_BOT_TOKEN` strip; exit 78 → 재발급 → 3회 상한 → `AuthFailed` + 수동 재시작으로 카운터 리셋(상한 제거 시 실패); refresh 태스크 stub 서버로 백오프/터미널 경로; loopback 리스너가 잘못된 `state`/5분 초과를 거절.
- acp `tests/stdio_contract.rs`: `BUZZ_BOT_TOKEN` 없으면 설정 오류 종료; 자식 env에 `BUZZ_BOT_TOKEN` 부재 + `BUZZ_TOKEN_BROKER_*` 존재. `token_refresh` 단위: exchange 성공 → 재-AUTH 전송 → 세대 증가; WS `OK auth false token_revoked` → exit 78; 네트워크 오류 → 백오프 상한.
- `e2e_managed_agent.rs` 토큰 모드 복제. mobile 위젯 테스트: `sig` 없는 JSON 파싱.
- B6: CLI 토큰 소스 우선순위 테이블(`BUZZ_BOT_TOKEN` > `BUZZ_ACCESS_TOKEN` > 브로커 > 키 폴백); 브로커 1회 재시도 후 exit 3; `git-credential-buzz`가 브로커에서 토큰을 받아 `username=token` 출력; admin `authorize_bearer` 운영자 아닌 Bearer → 403, 운영자 → 200(`AdminRole` 가드 제거 시 실패); git `Basic token:<bzb_>` push 성공, 만료 토큰 401.

수동 검증(사용자): dev 앱 Google 로그인 → 에이전트 생성 → 멘션 → 응답 → 에이전트가 `buzz --format compact channels list`와 git push를 토큰으로 성공 → Settings›Devices에서 다른 디바이스 로그아웃 → 에이전트 Stop → `access_tokens.revoked_at` 확인 → 관리 콘솔(Desktop) 접근. dev TTL 3분으로 exchange+재-AUTH가 연결을 유지한 채 일어나는지 로그 확인(`OK auth true` 두 번째 수신, 연결 id 동일).

### 5.2.1 Phase 1 상태 / handoff (2026-10-04)

Phase 1 구현 완료, **미커밋**(Phase 0 위 작업 트리). 리뷰 전. 플래그 off(`AUTH_TOKEN_ENABLED=false`)와 클라이언트 키 모드 동작은 그대로다 — 토큰 모드는 relay NIP-11이 `buzz_token_auth`를 광고하고(서버) 토큰/세션이 있을 때(클라이언트)만 켜진다.

**파일 (표면별, N = 신규, M = 수정)**
- 공용 기반: N `crates/buzz-sdk/src/signer.rs`(`EventSigner {Keys, Principal}` — 토큰 모드는 principal pubkey + sentinel sig draft, 서버 재스탬프 id와 일치); N `crates/buzz-token-broker/`(토큰 소스 우선순위 `BUZZ_BOT_TOKEN > BUZZ_ACCESS_TOKEN > 브로커`, std-only loopback 브로커 클라이언트, 1회 재시도, 4KB 상한, loopback 외 URL 거절); N `crates/git-credential-buzz/`(호스트 일치 시에만 `username=token`/`password=<token>`, 소스 + `git config buzz.tokenfile`); M `buzz-core/src/draft.rs`(`verify_served_event`: relay가 준 이벤트는 id 유효 + (sentinel 또는 유효 서명)); M `buzz-ws-client/src/{connection,lib}.rs`(`connect_with_token`, `authenticate_token`(재-AUTH 겸용), `publish_event_with_token`); M 루트 `Cargo.toml`/`Cargo.lock`.
- 서버(buzz-relay): N `identity/client_ip.rs`(`AUTH_TRUSTED_PROXY_CIDRS`, XFF 우→좌·`Forwarded` 워크, 손상된 hop은 가장 가까운 신뢰 프록시로, `unix` 리터럴로 UDS 신뢰); M `identity/{config,mod}.rs`; M `api/auth/{mod,oidc}.rs`(rate limit 키에 신뢰 프록시 클라이언트 IP); M `api/admin/auth.rs`(`authorize_bearer`: 모드 분기 전에 헤더 스킴으로 선택, user 토큰만, `resolve_admin_principal` 재사용, 토큰 실패 401·비운영자 403); M `api/git/transport.rs`(`Basic token:<t>`/`Bearer`, 401에 `Nostr`+`Basic realm="buzz"` 챌린지 둘 다, 봇은 owner ban 상속); M `nip11.rs`(`buzz_token_auth {version, bearer, oidc_providers}` 플래그 on일 때만); M `.env.example`.
- buzz-acp: N `src/identity.rs`(`AgentIdentity`, `/auth/me` 파싱, `EXIT_AUTH_TERMINAL=78`), N `src/token_refresh.rs`(만료 15분 전 exchange, 세대 펜스, 같은 WS 재-AUTH, 실패 분류), N `src/token_broker.rs`(127.0.0.1 `GET /token`, 스폰별 secret, 60/분, 8KB 요청/4KB 응답, 5초, 동시 16), N `src/relay/token_auth_tests.rs`; M `acp.rs`·`config.rs`·`edit_routing.rs`·`engram_fetch.rs`·`git.rs`·`lib.rs`·`pool.rs`·`relay.rs`·`run_task.rs`·`setup_mode.rs`, 테스트 `relay/recovery_*tests.rs`·`tests/stdio_contract.rs`; M `crates/sprig/src/main.rs`(argv0 `git-credential-buzz`).
- buzz-cli: N `src/client_token_tests.rs`; M `src/client.rs`(signer + Bearer, 401 터미널 코드 → exit 3), `src/lib.rs`(`build_client` 자격증명 선택), `commands/*`(pubkey 접근자 교체, 키 필요 명령은 exit 1 안내), `TESTING.md` §5a.
- Desktop Rust: N `src-tauri/src/auth/`(`api`, `bots`, `commands`, `credential`, `loopback`, `mod`, `pkce`, `redact`, `refresh`, `restore`, `watchdog`); M `app_state*.rs`, `relay.rs`, `relay/submit.rs`, `native_relay_client.rs`, `commands/{admin/*,agents,channels*,dms,identity*,messages*,profile,relay_members,teams/adopt/apply,workflows,workspace}.rs`, `managed_agents/{backend,env_vars/tests,hermes_profile_tests,reserved_env_keys,restore,runtime*,runtime_commands,storage}.rs`, `archive/*`, `persona_catalog.rs`, `team_catalog.rs`, `unread_catch_up.rs`, `lib.rs`.
- Desktop React: N `shared/api/{relayAuthFrame,tokenAuth}.ts`(+ `relayAuthFrame.test.mjs`), N `features/onboarding/ui/GoogleSignInButton.tsx`, N `features/settings/ui/AccountSettingsCard.tsx`(+ jsdom 테스트); M `relayClientSession.ts`, `readOnlyRelayClient.ts`, `useCommunityInit.ts`, `ProfileStep.tsx`, `SettingsPanels.tsx`.
- Mobile: M `shared/relay/nostr_models.dart`(`sig` → `String?`, toJson은 있을 때만), `shared/push/push_presentation_cache.dart`, 테스트 `nostr_models_test.dart`.

**테스트 실행과 결과** (§5.1.1 환경 그대로; `CARGO_TARGET_DIR`는 기본값)
```
cargo fmt --all -- --check                                   # ok
cargo clippy --workspace --all-targets -- -D warnings        # ok
cargo test -p buzz-relay --lib api::auth::live_tests -- --ignored --test-threads=1   # 11/11 (신규 admin Bearer, git Basic/Bearer 포함)
cargo test -p buzz-db identity -- --include-ignored --test-threads=1                # 10/10
cargo test --no-fail-fast -p buzz-core -p buzz-auth -p buzz-pubsub -p buzz-db -p buzz-relay   -p buzz-sdk -p buzz-token-broker -p git-credential-buzz -p buzz-ws-client -p buzz-cli -p sprig
                                                             # 2970 통과 / 6 실패 = §5.1.1의 기존 6건과 동일
cargo test -p buzz-acp                                       # lib 1044 통과 / 32 실패 = HEAD 워크트리에서도 같은 32건(bash 스크립트·타이밍)
cargo test --manifest-path desktop/src-tauri/Cargo.toml      # 3374 통과
desktop: tsc --noEmit ok, biome(변경 파일) ok, vitest/jsdom 163 통과
```
- admin `authorize_bearer`의 user-only 가드는 뮤테이션(가드 제거 → 테스트 실패)으로 반증 가능성 확인. 테스트는 roster에 봇 id까지 넣어 가드만이 403의 근거가 되게 한다.
- Desktop `cargo clippy -D warnings`는 Windows에서 수정하지 않은 파일(`buzz-terminal` 등)의 기존 경고로 실패 — 변경 파일에는 경고 없음.
- Mobile 테스트는 이 Windows 머신에 Flutter가 없어 **미실행**(Hermit shim 부트스트랩 실패). macOS에서 `just mobile-check mobile-test` 필요.

**결정 (계획이 열어둔 부분)**
- 토큰 모드 감지: relay NIP-11 `buzz_token_auth.bearer`(플래그 on에서만 존재). 세션은 커뮤니티(relay origin)별 — relay마다 계정 DB가 다르므로. keyring 키 `auth.refresh.<origin>`, access는 메모리.
- 공용 `EventSigner`: 클라이언트는 principal pubkey + sentinel sig로 이벤트를 만들고 서버가 재스탬프 — 기존 `nostr::Event` 경로·이벤트 id 계약 유지.
- relay가 돌려준 이벤트 무결성은 `verify_served_event`(sentinel 허용) — acp/CLI/Desktop의 `event.verify()` 중 relay-served 지점(huddle 지침, canvas, edit routing, workflow owner, archive 등)을 교체. 로컬 서명 검증은 유지.
- `git-credential-buzz`는 `BUZZ_RELAY_URL`(또는 `git config buzz.host`) 호스트에만 토큰을 준다(다른 원격으로 유출 방지). 서버는 플래그 on에서 401에 `Nostr`와 `Basic` 챌린지를 함께 보낸다.
- acp: exchange의 `invalid_token`·기타 4xx도 터미널(78)로 분류. `/auth/me` 거절도 78. 시크릿 키가 필요한 기능(NIP-44 engram/mem, NIP-AM, 암호화 observer, 커밋 서명)은 토큰 모드에서 로그와 함께 꺼짐.
- CLI: 키가 필요한 명령(`upload`/`media`, `mem`, `gifs`, NIP-44 draft)은 exit 1 + 사유(토큰 실패와 구분하려고 3이 아님).
- Desktop: 토큰 모드 등록 시 에이전트 레코드의 `pubkey`를 서버 bot id로 바꾸고 nsec·auth tag를 지운다(`bot_origin` 필드, §4.14). 별칭 맵(`auth-bots.json`)은 삭제. 등록은 (origin, agent) 잠금 + `auth-bot-adoptions.json` 저널로 중복 봇 없이 재개. relay로 가는 호출 지점(채널 추가, DM, 멘션)은 `adopt_named_agents`로 한 번 이동시킨 뒤 bot id를 쓴다. Hermes 프로필 디렉터리는 `buzz-<old>` → `buzz-<new>`로 rename(실패 시 새 프로필). bot 레코드는 자기 커뮤니티에서만 실행. 로그인/로그아웃/계정 삭제, 그리고 복원으로 계정이 바뀌면 창 리로드. UI 소켓은 만료 시 relay가 닫으면 재연결(제자리 재-AUTH는 acp만).

**후속 (Phase 1에서 하지 않음)**
- 서버가 principal/bot의 kind 0을 아직 발행하지 않음(§3.7) → 토큰 모드에서 이름/아바타 누락, 에이전트 자기 메시지가 bot id로 보임.
- Media/Blossom(`buzz upload`, Desktop 첨부), huddle, mesh/pairing은 Phase 2까지 토큰 모드 불가(명확한 오류).
- git push 실제 왕복 e2e(`e2e_git.rs` 토큰 버전)와 `e2e_managed_agent.rs` 토큰 모드 복제는 미작성 — live 테스트는 git 인증 분기(Basic/Bearer 통과, 잘못된 username·revoke 후 401, 챌린지 2종)까지만.
- 봇 삭제 시 세션이 메모리에 없는 다른 커뮤니티는 건너뜀. 닫힌 relay에서 봇 멤버십 필요(§5.1.1 그대로).
- acp 테스트가 `crates/buzz-acp/` 아래 잡파일(망가진 Windows 임시 경로 이름의 `.json`/`.ndjson`)을 다시 만든다 — 커밋 전 확인(`.concat`과 기존 잡파일은 v7에서 삭제).
- client_ip: 헤더 선택 스위치(XFF/Forwarded 중 하나만 신뢰)와 v4-mapped CIDR 정규화 재검토.
- `verify_served_event`의 sentinel 허용을 NIP-11이 토큰 인증을 광고하는 relay로 한정.
- 로그인 시 keyring 쓰기가 실패하면 origin별 토큰 모드 마커를 따로 영속화.
- 오프라인 로그아웃: 서버 revoke를 durable 재시도 큐로(지금은 이 기기에서만 잊음, 복원 실패 상태의 Sign out 포함).
- `buzz-acp run` 토큰 모드에는 refresh 태스크가 없음 → TTL(기본 1h)보다 긴 run은 미지원.
- 토큰 모드 welcome kickoff는 오프너를 보내지 않음(Desktop이 봇 토큰을 발급하면 실행 중 토큰이 revoke되므로 로컬 서명을 거부하고 경고만).
- Provider 백엔드는 토큰 모드에서도 아직 키 레코드를 만든다.
- Desktop 전체 `cargo fmt`는 수정하지 않은 `mesh_llm/recovery.rs` 파싱 실패로 중단 — 변경 파일만 개별 포맷.

**리뷰 반영 (v7, 2026-10-04)** — Phase 1 리뷰 블로커 6건 + Rule 3 + 즉시 수정 항목
- B1 admin Bearer는 `AdminAuth::Nip98`에서만; Disabled에서는 무시(뮤테이션 403). live 테스트 `admin_bearer_is_inert_under_disabled_admin_auth`.
- B2 회전된 refresh는 `/auth/refresh` 직후 세대 펜스 안에서 저장, 메모리에도 보관. 로그인/로그아웃/계정 삭제가 refresh 잠금을 잡음. 복원 대기 45s > 내부 요청 20s, 복원은 분리된 태스크.
- B3 복원은 refresh 잠금 아래 단일 실행, 호출자는 대기 후 재조회. auto-start도 대기. `useCommunityInit`이 `token-auth-changed`에서 계정이 바뀌면 리로드.
- B4 위 Desktop 결정 참조. kickoff는 토큰 모드에서 로컬 서명 거부. 삭제는 서버 먼저(404=삭제됨), 실패 시 로컬 유지.
- B5 acp: 새 연결(세대 증가) 시 대기 중 re-AUTH를 settle, `reauth()`에 AUTH_TIMEOUT.
- B6 broker: 에이전트 프로세스마다 lease(시크릿 1개), drop 시 해제, 가득 차면 거부(라이브 시크릿 축출 없음). CLI는 broker 429/5xx를 exit 2로.
- 기타: git-credential-buzz는 https(또는 relay 자체가 ws/http일 때 http)에만 토큰; CLI 명시적 `--private-key`가 토큰 env보다 우선(`--bot-token` CLI 플래그는 원래 없음, 회귀 테스트 추가); `buzz-acp run`이 broker 시작; exchange 타임아웃 8s(<10s replay 창); 봇 등록 잠금; Restoring 중 Sign out 유지; Google 버튼 스피너 aria-hidden; watchdog 재발급 실패·만료 직전 세션은 auth-failed로 park.
- 테스트: Tauri 3391 통과, acp lib 1054 통과/32 실패(HEAD와 동일 개수), buzz-cli 506, git-credential-buzz 8+5, live auth 12/12, jsdom 164, node 6866. fmt/clippy(workspace -D warnings)/tsc 통과. 각 가드는 뮤테이션 확인.

**재리뷰 반영 (v8, 2026-10-04)**
- N1 레코드 이동 후 `agents-data-changed` 발행. 옛 pubkey → bot id 맵(메모리 + 크래시 시 저널)을 `find_managed_agent_mut`와 Start/Stop/Delete/수정 명령이 따라감.
- N2 토큰 모드 welcome: 오프너와 provider 미준비 안내를 사용자 이름으로 리드 에이전트를 멘션해 게시(Desktop은 봇 토큰을 발급하지 않음). 실패는 토스트로 표시.
- 회전 후 keyring 쓰기 실패 시 저장된(소비된) refresh 삭제 → 다음 실행은 로그아웃 상태.
- 이동 순서: Hermes 프로필 rename → 레코드 저장 → 런타임 정리 → 옛 키 삭제 → UI 이벤트. 시작 시 `sweep_adoption_journal`이 미완료 이동을 정리.
- 삭제: 펜스 → 프로세스 정지 → 서버 삭제 → 로컬 삭제(watchdog 재등록 차단). 순서 테스트 추가.
- `adopt_named_agents`는 다른 커뮤니티의 bot 레코드를 거부. persona 이름/아바타 동기화는 bot 레코드에 `sync_bot_profile`.
- 로그인 시 키로 실행 중인 에이전트는 "Restart required"(`sign_in`) 표시 — 자동 재시작 안 함.
- replay 창 상수를 `buzz-core` `principal.rs` 하나로 공유(relay TTL, acp). acp의 옛 bash 테스트가 크레이트 폴더에 잡파일을 만들던 경로 문제 수정.
- 테스트: Tauri 3408 통과/0 실패, acp lib 1054/32(기존과 같은 개수), live auth 12/12. fmt/workspace clippy -D warnings/tsc 통과.

**후속 추가 (v8)**
- persona/team 스냅샷 가져오기가 토큰 모드에서도 키 에이전트를 만들고 engram을 키로 서명.
- Restoring 중 Sign out은 서버 revoke 없이 로컬에서만 잊음.
- broker 60/분이 MCP와 공유 — CLI/MCP 토큰 캐시 검토.
- `create_bot`과 저널 기록 사이 크래시 시 서버 봇 고아.
- `agents_auth_reset_tests`와 adopt 다른-커뮤니티 거부·restart-required 호출 지점 테스트는 Windows에서 미실행(cfg(not windows) / mock 앱 필요). 기존 bots 테스트의 `build_app_state()`는 Windows 게이트 없음 — Windows CI 위험.
- welcomeKickoff node 테스트는 메시지 빌더와 실패 보고만 검사, 훅의 호출 자체는 미검증.

**최종 (v9, 2026-10-04)** — 3차 리뷰 READY(정확성 9).
- closer도 오프너와 같은 사용자 명의 fallback을 사용 → kickoff 도중 Google 로그인해도 closer가 사용자 이름으로 게시, 토스트 반복 없음(node 테스트 2).
- `resolve_managed_agent_poster`: 사용자 자격증명은 사용자 명의 분기에서만 조회 → 키 모드 에이전트 게시는 recovery 모드에서도 동작, Blocked 토큰 커뮤니티는 여전히 에이전트로 서명하지 않음(Rust 테스트 2).
- `stop_managed_agent_for_delete` pubkey 비교 대소문자 무시.
- 게이트: Tauri 3410/0, workspace clippy -D warnings 통과, tsc·biome 통과, onboarding node 104/0, acp lib 1054/32(기존과 같은 개수), live auth 12/12.
- 후속: 키 모드 커뮤니티 간 경합(B에서 키 모드 시작이 A의 이동과 동시에 일어나면 슬롯을 잡지 않음); 다른-커뮤니티 거부·Restart required 호출 지점은 Windows에서 미검증; 기존 bots 테스트의 `build_app_state()`는 Windows 게이트 없음(Windows CI 위험).
- 커밋 분리: Phase 0 전용 파일 ~48, Phase 1 전용 ~158, 양쪽이 수정한 파일 ~13(`.env.example`, 루트 `Cargo.toml`/`Cargo.lock`, buzz-core `lib.rs`/`draft.rs`/`principal.rs`, relay `identity/mod.rs`·`identity/config.rs`, `api/auth/{mod,oidc,bots,live_tests}.rs`, 이 문서). Phase 0 스냅샷이 없어 겹치는 파일은 hunk 단위 분리가 필요. `CLAUDE.md`는 stage하지 않는다.

### 5.3 Phase 2 — CLI 로그인, Mobile, Web, 운영자 관리, 그리고 Phase 3 선행 의존 제거 (B4)

범위
- buzz-cli `auth` 명령(사람 로그인, 봇 관리). Mobile 로그인/secure storage/Bearer/토큰 AUTH/draft/Account·Devices/pairing 삭제. (CLI 토큰 모드·`git-credential-buzz`·admin Bearer는 Phase 1로 이동, B6.)
- **web/** Google 로그인 세션(§4.17).
- **운영자 관리**: `/auth/operators` 엔드포인트(기존 `relay_operators` 스토어 + `AdminRole`, B5), `RELAY_OPERATOR_PUBKEYS`/`RELAY_OWNER_PUBKEY` config 우선순위 제거(부트스트랩은 Phase 0부터 존재, §3.3 B8).
- **B4 의존 제거(Phase 3이 컴파일되기 위한 선행)**:
  - `audio/handler.rs:495-576` NIP-42 → 토큰 AUTH 프레임; `:2984,:3429` → `relay_principal` 스탬프.
  - `api/bridge.rs:799,2565-2628,1426` → `relay_principal`.
  - `invite_token.rs:111` → `INVITE_SIGNING_SECRET`.
  - `buzz-media/auth.rs` Blossom → Bearer.
  - `buzz-workflow/executor.rs:994-998` → 토큰/내부 경로.
  - `web/` `nostr-signer.ts`, `nip98.ts`, `git-client.ts:57`, `invite-api.ts`.
  - ~~나머지 `relay_keypair` 서명 사용처 전부 → `relay_principal` 스탬프~~ **(v10에서 취소, §5.3.1)**: relay principal = relay 서명 키이므로 relay 저작 이벤트는 계속 서명된다.
- Phase 2 이후 `relay_keypair`는 relay 자신의 서버 신원으로 남는다: relay 저작 이벤트 서명(principal과 같은 저자), 외부 S2S 인증(push 게이트웨이 NIP-98, operator listener, push lease, artifact), 그리고 NIP-42 챌린지 검증(이것만 Phase 3 삭제).

테스트
- CLI `auth login` loopback 왕복(가짜 provider), `session.json` 소스 순위, 401 terminal → exit 3.
- 운영자 관리: `PUT /auth/operators` 운영자만 가능, moderator 역할 부여·조회; config 우선순위 제거 후 로스터만으로 `AdminRole` 해석.
- audio: 토큰 AUTH로 룸 join; 만료 → close.
- invite: 시크릿으로 발행·검증 왕복; 시크릿 변경 시 구 초대 무효.
- mobile 위젯: Google 로그인 버튼 → 가짜 auth 결과 → 상태 전이; 자동 로그인 성공/실패; Devices 원격 로그아웃 후 목록 갱신.
- web Playwright: 로그인 리다이렉트 왕복(가짜 provider), 쿠키 refresh.

수동 검증: `buzz auth login`(브라우저 열림) → `buzz --format compact channels list` → 에이전트 내부 `buzz` 호출(브로커). iOS 시뮬레이터 Google 로그인 → 메시지 → 재시작 후 유지. 운영자 콘솔 Bearer 접근. git push 토큰.

### 5.3.1 Phase 2 상태 / handoff (2026-10-04)

Phase 2(모바일 제외) 구현 완료, **미커밋**(909bfce84 위 작업 트리). 리뷰 전. 모든 서버 변경은 `AUTH_TOKEN_ENABLED`에 묶여 있고 플래그 off 동작은 그대로다. 클라이언트(Desktop/CLI/web)는 Phase 1처럼 relay NIP-11 `buzz_token_auth`가 있고 세션/토큰이 있을 때만 토큰 경로를 탄다.

**파일 (표면별, N = 신규, M = 수정)**
- 서버 프로필(kind 0): N `crates/buzz-relay/src/identity/profile.rs`(principal/봇 kind 0 서버 스탬프 발행, 내용 비교로 멱등, 같은 초 tie-break 회피); M `identity/ws.rs`(토큰 AUTH 성공 후 그 커뮤니티 reconcile 스폰), `api/auth/session.rs`(`PATCH /auth/profile` 후 발행), `api/auth/bots.rs`(봇 생성·`PATCH /auth/bots/{id}/profile` 후 발행); M `buzz-db/src/store/identity/{principal,mod}.rs`(`principal_profile_communities`, `latest_principal_profile_event`, 상한 `MAX_PROFILE_COMMUNITIES=1000`).
- 서버 relay 아이덴티티: M `buzz-db` `ensure_relay_principal(key)`(relay 서명 키를 relay principal로, 다른 id면 재키잉 + `relay_operators.added_by` 추종, advisory lock); M `identity/mod.rs` `init_relay_principal`(main.rs·live 테스트 공용 기동 경로), `main.rs`.
- 운영자: M `buzz-relay/src/config.rs` `with_token_mode_operator_policy`(플래그 on이면 `RELAY_OPERATOR_PUBKEYS`/`RELAY_OWNER_PUBKEY` 무시 + 경고, 로스터만).
- 초대: M `identity/config.rs`(`INVITE_SIGNING_SECRET`, 플래그 on에서 필수, Debug 마스킹), `invite_token.rs`(`derive_invite_key_from_secret`, 별도 라벨), `api/invites.rs`(`invite_key` 선택, mint·claim·accept-policy Bearer 분기 — user 세션만, 봇 403).
- web 지원: M `router.rs`(플래그 on에서 `/auth/cb`를 web SPA로 서빙).
- huddle 오디오: N `audio/token_auth.rs`(admit된 소켓의 같은 principal 토큰 교체 `auth_ok`/`auth_error`); M `audio/handler.rs`(`{"type":"auth","token"}` 형식, 공용 admission, 토큰 바인딩·deadline·revoke 레지스트리), `audio/mod.rs`, `identity/ws.rs`(`TokenSession` close 프레임 경로별); Desktop M `huddle/relay_api.rs`(토큰 모드면 세션 access로 AUTH, 30초마다 회전 감지 후 재전송), `huddle/pipeline.rs` + N `pipeline_stt_poster_tests.rs`(STT 전사 게시를 principal+Bearer로).
- media: N `api/media/token_auth.rs`(Bearer 게이트 = bridge와 동일 조건, 내부 업로드 증명 kind 24242 sentinel), M `api/media.rs`(업로드·GET·HEAD Bearer 우선, 기존 Blossom 경로 그대로); M `buzz-cli/src/client.rs`(+tests, `upload`/`media` Bearer; `MEDIA_REQUIRES_KEY` 삭제); Desktop M `commands/media.rs`(업로드·미디어 프록시·다운로드·persona 카드가 커뮤니티 자격증명으로 Bearer/Blossom 선택). `buzz-media` 크레이트는 무변경.
- workflow: M `buzz-workflow/src/executor.rs` `relay_auth_header`(`BUZZ_BOT_TOKEN` > `BUZZ_API_TOKEN` Bearer > dev `X-Pubkey`).
- buzz-cli `auth`: N `src/auth_session.rs`(origin별 `session.json`, OS keyring refresh, 회전), N `src/auth_loopback.rs`(PKCE, state, loopback `/cb`), N `commands/auth.rs` + `auth_tests.rs`; M `lib.rs`(auth 그룹, `build_client_with_session`), `commands/mod.rs`, `Cargo.toml`(`keyring` 4, `webbrowser` 1, `zeroize` — 모두 lock에 이미 존재), `TESTING.md` §5b.
- web: N `web/src/shared/auth/{pkce,web-session,token-mode,auth-mode,session}.ts` + 테스트 3, N `shared/auth/ui/{AccountControl,AuthCallbackPage}.tsx`, N `app/routes/auth.cb.tsx`; M `routes.ts`, `routeTree.gen.ts`, `routes/root.tsx`, `features/invite/{invite-api.ts,ui/InvitePage.tsx}`, `features/repos/git-client.ts`, `shared/lib/nostr-client.ts`, `package.json`(`test` 스크립트).
- 테스트 신규: `api/auth/live_tests/{profile,invites,audio}.rs`, `router_tests.rs`(`/auth/cb`), `invites.rs` `invite_key_tests`, `config.rs` 운영자 정책, `identity/config.rs` 시크릿 필수.
- `.env.example`: `INVITE_SIGNING_SECRET`(플래그 on 필수), 토큰 모드 운영자 정책 설명.

**테스트 실행과 결과** (§5.1.1 환경 그대로)
```
cargo fmt --all -- --check                                   # ok
cargo clippy --workspace --all-targets -- -D warnings        # ok
cargo test -p buzz-relay --lib api::auth::live_tests -- --ignored --test-threads=1   # 20/20 (신규 profile, invites, audio 5, media 포함)
cargo test -p buzz-db identity -- --include-ignored --test-threads=1                # 10/10 (relay principal 재키잉 포함)
cargo test --no-fail-fast -p buzz-core -p buzz-auth -p buzz-pubsub -p buzz-db -p buzz-relay \
  -p buzz-sdk -p buzz-token-broker -p git-credential-buzz -p buzz-ws-client -p buzz-cli -p sprig -p buzz-workflow -p buzz-media
                                                             # 3350 통과 / 6 실패 = §5.1.1 기존 6건과 동일
cargo test --manifest-path desktop/src-tauri/Cargo.toml      # 3429 통과 / 0 실패
web: tsc --noEmit ok, biome(변경 파일, 루트 node_modules의 biome) ok, vite build ok, node --test 18/18, file-size·pubkey-truncation ok
```
- 뮤테이션 확인(가드 제거 → 테스트 실패): PATCH 후 발행, AUTH 후 reconcile, 초대 user-only 가드, 오디오(레지스트리 insert, 플래그, rebind, deadline watch, desktop 키 폴백), media(Bearer 게이트, 멤버십), CLI(세션 우선순위, 회전 저장, 로그아웃 실패 시 유지), web(single-flight, 세대 펜스, 재시도 상한, state 검사, 세션 요구, 로그아웃 유지).
- 기존 실패 추가 관찰: `audio::...cw10_full_handler_committed_join_produces_exactly_one_leave_event`(DB 필요 테스트)는 HEAD `handler.rs`에서도 같은 방식으로 실패.
- Playwright(web)는 pnpm 부재로 미실행. S3/MinIO가 없어 media 실제 업로드 왕복은 미실행(인증 통과 후 저장 실패까지 확인).

**결정 (계획이 열어둔 부분)**
- **relay principal = relay 서명 키의 공개키.** relay pubkey는 DB 쿼리(39000 등 relay 저작 이벤트), NIP-11 `self`, push lease, identity archive에 쓰인다. 랜덤 id로 바꾸면 기존 relay 이벤트가 전부 다른 저자가 된다. 같은 id면 서명과 스탬프가 같은 저자를 만든다. Phase 0/1의 랜덤 relay row는 기동 시 재키잉된다.
- **계획 편차 — `relay_keypair` 일괄 전환(B4 마지막 항목)을 하지 않음.** relay 키는 클라이언트 신원이 아니라 서버 자신의 신원이며 외부 S2S 인증(push 게이트웨이 NIP-98, operator listener, push lease, artifact)에 계속 필요하다. relay principal이 그 키와 같으므로 relay 저작 이벤트는 계속 서명된다(키 모드 클라이언트도 검증 통과). 따라서 **Phase 3은 `RELAY_PRIVATE_KEY`/`relay_keypair`를 삭제하지 않는다**(§5.4 체크리스트의 해당 행은 무효). Phase 3이 지우는 것은 클라이언트 키 경로(NIP-42/98/OA/FI)다.
- **kind 0**: 서버 스탬프(pubkey = principal, sentinel sig), principal의 `users` row가 있는 활성 커뮤니티마다 `replace_addressable_event` → 기존 kind 0 side effect(users projection) → `dispatch_persistent_event`. 내용이 같으면 발행하지 않음. 트리거는 프로필 쓰기(인라인)와 토큰 WS AUTH 성공(스폰, AUTH 지연 없음). 실패는 다음 AUTH의 reconcile이 수렴시킨다(principals row가 원본). 봇은 `"bot": true`. 계정 생성 시점에는 users row가 없으므로 첫 AUTH에서 발행된다.
- 운영자 config 우선순위 제거는 플래그 on에서만(config 로드 단일 지점). 플래그 off는 그대로.
- `INVITE_SIGNING_SECRET`은 플래그 on에서만 필수(없으면 기동 실패). 키는 `sha256(secret || "buzz-invite-secret-v1")` — 시크릿 키와 relay 키 파생이 서로의 코드를 검증하지 않는다. v2(DB) 초대 코드는 키와 무관해서 계속 유효; v1 코드와 정책 receipt만 무효.
- 초대 mint/claim/accept-policy의 Bearer는 **user 세션만**(Telegram: 봇은 초대 링크로 들어가지 않고 owner가 추가). mint는 기존 owner/admin 역할 검사 그대로.
- 오디오: 토큰 소켓은 루트 WS와 같은 바인딩 레지스트리·deadline 감시·revoke를 공유, 같은 principal 토큰 교체 지원(1h 만료로 허들이 끊기지 않게). 토큰 모드 에이전트의 TTS 음성은 미지원(Desktop이 봇 토큰을 새로 발급하면 실행 중 토큰이 revoke됨).
- media: Bearer 게이트는 bridge와 같은 조건(플래그 on + NIP-FI off), 읽기(GET/HEAD)도 Bearer 허용(토큰 모드 Desktop 미디어 프록시가 서명 불가하므로).
- CLI 자격증명 순서: 명시 토큰 env(`BUZZ_BOT_TOKEN` > `BUZZ_ACCESS_TOKEN` > 브로커) > 개인키(argv/env) > 저장된 세션. 세션을 키보다 아래에 둔 이유는 에이전트에 주어진 신원을 사람 로그인이 덮지 않게(Hermes `.env` 버그 클래스). refresh는 OS keyring(service `buzz-cli`, account `refresh:<origin>`), 없으면 `session.json`(0600) 폴백 + 경고(`gh`와 같음). 회전된 refresh를 저장 못 하면 소비된 사본을 지우고 exit 3. 로그아웃 네트워크 실패는 세션 유지 + exit 2.
- web: access는 JS 메모리, 부팅·만료 5분 전 쿠키 refresh(single-flight, 세대 펜스, 400/401 → 로그아웃, 그 외 5s~300s 백오프 후 중단). 로그아웃 서버 호출 실패 시 세션 유지 + 오류 표시. 초대 claim은 토큰 모드에서 세션 필수(임시 키 폴백 없음).
- workflow `AddReaction`: 헤더만 토큰화. 대상 `/api/messages/{id}/reactions`는 relay에 라우트가 없다(기존 결함) — 후속.

**후속 (Phase 2에서 하지 않음)**
- `with_token_mode_operator_policy`의 테스트는 메서드 직접 호출 — `from_env`에서의 호출 자체는 env 변경 없이 고정하기 어려워 미바인딩.
- HTTP 전용 토큰 클라이언트(CLI만 쓰는 사용자)는 WS AUTH가 없어 kind 0 reconcile이 일어나지 않는다(프로필 PATCH 시에는 발행됨). bridge Bearer에 (커뮤니티, principal) 프로세스 캐시와 함께 reconcile을 붙일지 검토.
- 봇 아바타 현지화(`desktop/src-tauri/src/relay/profile_avatar.rs`)는 에이전트 키로 서명 — 토큰 모드 봇은 아바타 업로드 불가. 서버 kind 0은 이름만 보장.
- 오디오: 토큰 교체가 연결당 operations 버킷에 포함되지 않음(동시 1회로만 제한); 에이전트 TTS 음성 경로.
- media 실제 업로드 왕복(`e2e_media.rs` 토큰판) — MinIO 환경 필요.
- web: 다중 호스트 배포에서 다른 커뮤니티 호스트의 web 로그인은 단일 `AUTH_PUBLIC_URL` 때문에 redirect 거절; 오프라인 로그아웃 durable 재시도 없음; 10초 넘게 벌어진 두 탭의 같은 쿠키 refresh는 재사용 탐지 위험.
- CLI: 동시 refresh 잠금 없음(10초 replay 캐시만), `delete-account` 미노출, 실제 relay+가짜 provider live 테스트 없음.
- workflow `AddReaction`을 `ActionSink` 내부 경로로(지금은 존재하지 않는 HTTP 라우트 호출).
- Phase 1 후속 중 persona/team 스냅샷 가져오기의 키 에이전트 생성은 Windows에서 앱 없이 검증이 어려워 미착수.

**리뷰 반영 (v11, 2026-10-05)** — Phase 2 리뷰 블로커 B1–B3 + 선택 항목. 이 절이 위 "결정"·"후속"의 해당 항목보다 우선한다.
- **B1 운영자 config**: `Config`를 변경하지 않는다(`with_token_mode_operator_policy` 삭제). 대신 `Config::admin_config_operator_pubkeys()`/`admin_owner_fallback_pubkey()`가 플래그 on에서 비어 있고, admin 해석 지점(`admin/auth.rs` `lookup_admin_principal`, `admin/mod.rs` 운영자 목록·`config_operator_exists`·`is_config_backed_pubkey`)만 이를 쓴다. `RELAY_OWNER_PUBKEY`는 그대로 NIP-43 owner 부트스트랩과 `BUZZ_REQUIRE_RELAY_MEMBERSHIP` 기동 검사(`Config::check_relay_membership_owner`, main.rs가 호출)에 쓰인다. `/operator/*` 커뮤니티 프로비저닝(`api/operator.rs`, `handlers/community_provisioning.rs`)은 키 서명 API로 **Phase 3까지 `RELAY_OPERATOR_PUBKEYS` 유지**(토큰 경로 없음 — 후속). 테스트: 실제 `from_env`를 env(플래그 on + 멤버십 필수 + owner + operator)로 로드해 owner/operator 필드 보존, admin 해석은 비어 있음, 플래그 off는 기존대로(가드 제거 시 실패 확인).
- **B2 kind 0 경합**: 발행은 `Db::lock_principal_profile_publish(community, principal)` — `pg_advisory_xact_lock(sha256(community‖principal) 앞 8바이트)`를 잡은 트랜잭션 안에서 principal row·최신 kind 0·`users` row 유무를 읽고, insert·side effect·dispatch를 마친 뒤 commit(드롭 시 rollback으로 해제). 늦게 도착한 발행자는 항상 최신 프로필을 읽으므로 `max(now, prev+1)`이 낡은 이름을 올리지 않는다. 같은 연결의 토큰 재-AUTH 성공 후에도 reconcile → 실패한 발행이 1시간 안에 수렴(Rule 1). 테스트: 잠금을 쥔 채 발행을 스폰 → 400ms 대기 확인 → principal 이름 변경 → 해제 → **그 발행자가 쓴 이벤트**가 새 이름(잠금 제거 시 실패 확인); 발행 없이 바뀐 이름이 재-AUTH로 수렴(재-AUTH reconcile 제거 시 실패).
- **B3 flaky**: `profiles()`가 id로 중복 제거(백필/라이브 겹침), `users.display_name`은 기한 내 폴링. 전체 live 스위트 5회 연속 결과는 아래.
- 선택 반영: PATCH·봇 생성/프로필의 fan-out은 스폰(동시 4, `for_each_concurrent`); **AUTH/재-AUTH reconcile은 `users` row가 있는 커뮤니티만**(읽기만 하는 커뮤니티에 Google 이름 공개 안 함), 대신 **토큰 principal의 첫 accepted 쓰기**(WS EVENT, `POST /events`)에서 그 커뮤니티에 발행(프로세스별 10분/10만 캐시로 중복 억제, 실패 시 캐시 무효화) — 첫 메시지 경로와 CLI 전용 사용자 후속도 이것으로 해결; 봇은 AUTH의 owner 링크가 `users` row를 만들므로 AUTH reconcile로 발행. web `safeReturnTo`: 백슬래시·제어문자 거부, `URL`로 정규화 후 같은 origin·`/auth` 경로 아님만 허용. CLI: acp가 모든 에이전트 자식에 `BUZZ_DISABLE_STORED_SESSION=1`(토큰·키 모드 모두, 다른 env 적용 후) → `buzz`는 저장된 로그인을 쓰지 않고 exit 3, `buzz auth login/logout`은 거부. 계획 문서 §5.3·§5.4 정정.
- Rule 3 테스트 추가: `relay_operators.added_by` 재키잉 추종; relay 키가 이미 user/bot이면 사전 거부(`InvalidData`, 기존 사후 비교는 도달 불가라 사전 검사로 교체); 오디오 토큰 ban 게이트 — banned principal 거부, banned owner의 봇 거부 **그리고 owner 링크(`users` row) 미기록**(결과만으로는 뒤의 final admission이 같은 거부를 내므로, 게이트의 고유 효과인 "부수효과 전 거부"로 반증); NIP-FI assertion이 있는 오디오 토큰 프레임 → `auth-required: unsupported auth`; 오디오 재-AUTH single-flight; 발행 결정의 disabled/relay-kind/`users` row 범위(`desired_profile`); `invite_bearer`의 NIP-FI≠Off 게이트. 각 가드 제거 시 실패 확인.
- 거절한 선택 항목: `profile_reader`의 reader pool 사용. 이 코드베이스의 replica 읽기는 `route_read` 펜스를 거쳐야 하고(원시 replica pool은 테스트 전용), 지연된 replica는 방금 만든 `users` row를 놓쳐 발행 대상에서 빠뜨린다. writer의 `Authentication` 연산으로 유지.
- 후속 추가: `INVITE_SIGNING_SECRET`은 이전 시크릿 검증 창이 없다 — 회전 즉시 미사용 v1 코드와 정책 receipt가 무효(v2 DB 초대는 무관). 필요 시 `INVITE_SIGNING_SECRET_PREVIOUS`로 검증만 허용하는 창을 추가. `/operator/*` 프로비저닝 토큰 경로(Phase 3 전). 오디오 ban 게이트의 principal 단독 경로는 결과·부수효과 모두 final admission과 같아 개별 반증 불가(owner 연쇄는 반증됨).
- 테스트 결과(v11): fmt·workspace clippy -D warnings 통과; live auth 스위트 **5회 연속 22/22**; buzz-db identity 10/10; 워크스페이스 13크레이트 3354 통과 / 6 기존 실패 + 간헐 `telemetry::tests::trace_context_lookup_does_not_enable_callsites`(전역 callsite interest 경합 — 909bfce84 워크트리에서도 5회 중 3회 실패, 단독 실행은 통과); buzz-acp lib 1054/32(기존과 동일); Tauri 3429/0; web tsc·biome·vite build·node 18/18·file-size·pubkey 통과.


- **재리뷰 반영 (v11.1)**: kind 0 발행 앞에 프로세스 전역 `tokio::sync::Semaphore`(`PublishSlots`, `max(1, 풀 최대/4)`) — 커넥션보다 먼저 permit을 잡고 잠금 해제까지 보유해, 재접속 폭주 시 발행자들이 잠금 커넥션을 쥔 채 두 번째 커넥션을 기다리며 풀을 고갈시키는 hold-and-wait를 막는다. live 테스트: permit 전부 + advisory lock 보유 상태에서 발행 3개가 커넥션을 잡지 않고 대기(세마포어 제거 시 사용 중 커넥션 1→4로 실패). B1 config 테스트의 env 변경은 기존 `NIP_FI_ENV_LOCK` 아래에서 실행.

**Phase 2b (mobile), deferred** — 이 Windows 머신에 Flutter가 없어 빌드·테스트 불가. macOS에서 별도 구현. 서버는 이미 준비됨(`client=mobile`, `AUTH_MOBILE_REDIRECT_SCHEMES` 기본 `xyz.block.buzz`, refresh 본문 반환, WS 토큰 AUTH, Bearer bridge/media/invite, kind 0 서버 발행).
- `shared/auth/auth_provider.dart`: nsec 상태 → `{principalId, accessToken, accessExpiresAt}`(Riverpod 메모리), refresh는 `flutter_secure_storage`(origin별 키), 앱 시작 시 refresh 자동 로그인, 만료 5분 전 refresh(single-flight, 세대 펜스, 401 → 로그인 화면; Rule 6 "다시 로그인" 경로 보장). 감지는 NIP-11 `buzz_token_auth.bearer`.
- 로그인: `flutter_web_auth_2`(iOS `ASWebAuthenticationSession`, Android Custom Tabs) + PKCE S256, `redirect_uri=xyz.block.buzz://auth/cb`, state 검사, `POST /auth/oidc/complete`. iOS `Info.plist`/Android manifest에 scheme 등록. 워크트리별 debug identity(`scripts/mobile-worktree-overrides.sh`)와 scheme 충돌 여부 확인.
- `relay_session_auth.dart` → `Authorization: Bearer`; `relay_session.dart:146-179,500` → `["AUTH",{"token"}]`, 재연결 시 현재 토큰, `OK auth false token_expired` → refresh 후 재연결; `signed_event_relay.dart` → draft(서명 없음, 서버 스탬프). 미디어 업로드는 Bearer(서버 Phase 2 완료). 허들 오디오는 `{"type":"auth","token"}` + 회전 시 같은 소켓에 재전송(`auth_ok`).
- `features/settings`: Account(전역 프로필 PATCH, 아바타 업로드 → `avatar_url`), Devices(목록·원격 로그아웃·"모든 다른 기기 로그아웃"), "모든 봇 토큰 회수", 계정 삭제. `features/pairing`(NIP-AB) 삭제, `shared/crypto/{nip44,ecdh,nip_oa}.dart`는 Phase 3.
- 테스트(위젯, `ProviderScope` 오버라이드): 로그인 버튼 → 가짜 auth 결과 → 상태 전이; 자동 로그인 성공/실패; refresh 401 → 로그인 화면; Devices 원격 로그아웃 후 목록 갱신. 게이트 `just mobile-install mobile-check mobile-test`, iOS 시뮬레이터에서 Google 로그인 → 메시지 → 재시작 후 유지 수동 검증.

### 5.4 Phase 3 — 키 인증·서명 제거

범위
- relay: NIP-42/98/OA/FI 분기 삭제, 서명 검증 삭제, AUTH 챌린지 송신 중단, 클라이언트(토큰) 이벤트의 sentinel `sig` 와이어 출력·DB 쓰기 중단(relay 서명 이벤트는 실제 서명이므로 `sig` 유지 — v10), ~~`relay_keypair` 필드 삭제~~(v10: 유지, §5.3.1), 클라이언트 키 관련 config 제거, gift wrap 예외 삭제·기존 1059 `deleted_at`.
- buzz-auth nip42/nip98/nip_fi 삭제, `AuthContext.pubkey` 삭제. buzz-pubsub replay 모듈 삭제.
- ws-client/sdk/acp/cli/desktop/mobile 키 API 삭제, `Keys` 의존 제거.
- `docs/nips/NIP-OA.md`, `NIP-FI.md` 삭제.

**삭제 체크리스트(B4)** — 각 항목이 Phase 2에서 이미 토큰화되어 있어야 하며, Phase 3 PR 설명에 체크로 남긴다:

| 파일 | Phase 2에서 대체된 것 |
|------|----------------------|
| `api/admin/auth.rs:371-434,459` | `authorize_bearer` |
| `api/git/transport.rs:160-180` | Basic/Bearer |
| `audio/handler.rs:33,370-400,495-576,2984,3429` | 토큰 AUTH, `relay_principal` |
| `api/bridge.rs:98-176,799,1426,2565-2628` | Bearer, `relay_principal` |
| 나머지 `relay_keypair` 사용처(총 24파일 97지점): `side_effects.rs`, `operator_listener.rs`, `workflow_sink.rs`, `push_runtime.rs`, `push_lease.rs`, `identity_archive.rs`, `moderation_notices.rs`, `artifact.rs`, `thread_roots.rs`, `thread_window.rs`, `api/git/settings.rs`, `nip11.rs`, `handlers/req.rs`, `mesh_boot.rs`(Phase 4 삭제) | `relay_principal` 스탬프 (컴파일러가 누락을 잡지만 체크리스트에 명시) |
| ~~위 행~~ **무효(v10)**: relay principal = relay 키(§5.3.1). relay 키는 서버 신원·S2S 인증으로 유지하며 Phase 3에서 삭제하지 않는다 | - |
| `invite_token.rs:111` | `INVITE_SIGNING_SECRET` |
| `handlers/auth.rs` NIP-42 분기, `connection.rs:526 generate_challenge` 송신 | 토큰 AUTH |
| `handlers/ingest.rs:2402-2420` | id 재계산 |
| `nip98.rs`, `nip_fi_*.rs`, `api/nip_fi.rs` | 없음(삭제) |
| `buzz-media/src/auth.rs` kind 24242 | Bearer |
| `buzz-workflow/src/executor.rs:994-998` | 토큰 |
| `web/src/shared/lib/{nostr-signer,nip98}.ts`, `git-client.ts:57`, `invite-api.ts` | 세션 |
| `crates/buzz-test-client/tests/nip42_host_binding_live.rs` | `auth_token_live.rs`의 host 바인딩 케이스로 대체 |

테스트
- e2e 스위트(`e2e_relay.rs`, `e2e_media.rs`, `e2e_nostr_interop.rs`의 NIP-50/NIP-10 부분, `e2e_git.rs`…)를 토큰 기반으로 재작성. NIP-17 부분 삭제.
- 회귀 가드: 서명 없는 draft 수락; `["AUTH", <event>]` 구 형식 → `OK false "auth-required: unsupported auth"`; `sig` 필드가 응답에 없음.

리스크: 가장 큰 삭제. 크레이트 단위 커밋(relay → auth/pubsub → ws-client/sdk → acp → cli → desktop → mobile → web)로 각 커밋 빌드 유지.

### 5.5 Phase 4 — 정리·문서

- 마이그레이션: `events.sig` drop, `api_tokens`/`pubkey_allowlist`/`join_policy_acceptances`/NIP-FI 테이블 drop, `users.nip05_handle` drop, `_operator_global_tables` 정리.
- 크레이트 삭제: `git-sign-nostr`, `buzz-pair-relay`, `buzz-pairing-cli`, mesh 관련(`audio/mesh.rs`, `api/mesh_demo.rs`, `buzz-relay-mesh` 등)(확정 Q8).
- `BuzzEvent` 타입 교체(권고: 한다. `nostr` 의존 축소, §2.1 불변식 해제). 교체 후 `PrincipalId::generate()`는 그대로 두되 x-only 검증은 선택이 된다(기존 id 호환 유지).
- `kind.rs` 상수 정리, `kinds.ts`, `nostr_models.dart` 동기. `executor.rs:194` npub 필터 → principal 필터.
- desktop `key_backup.rs`, `hpke_key_backup.rs`, `identity_storage.rs` 삭제.
- 문서(§4.18), `.env.example`, `VISION.md`.

### 5.6 Phase 요약

| Phase | 머지 후 상태 | 삭제 | 사용자 가시 변화 |
|-------|-------------|------|------------------|
| 0 | 키+토큰 병행, OIDC 엔드포인트 존재, 플래그 off | 없음 | 없음 |
| 1 | Desktop Google 로그인, 봇 토큰, acp exchange+재-AUTH, CLI 토큰 모드, `git-credential-buzz`, admin/git Bearer 분기 | 없음 | 로그인 화면, 에이전트 재등록 안내 |
| 2 | CLI 로그인/Mobile/Web/운영자 관리/audio/invite/media/bridge 토큰화, config 운영자 우선순위 제거 | pairing, NIP-98 사용처 | 모바일·웹 Google 로그인, 기존 초대 링크 무효 |
| 3 | 토큰만 | NIP-42/98/OA/FI, 서명, gift wrap | 구 클라이언트 접속 불가 |
| 4 | 정리 완료 | 컬럼/테이블/크레이트 | 없음 |

---

## 6. 보안 분석

### 6.1 토큰이 acp env/메모리에 있는 문제
- `BUZZ_BOT_TOKEN`은 Desktop → acp 한 홉만 env. acp는 `SecretString`(zeroize) 보관, 자식 스폰에 `Command::env_remove`. 자식에는 브로커 URL+스폰별 secret만. 자식이 토큰을 얻어도 1h `bzb_`이며 Stop 시 revoke.
- 같은 사용자의 다른 프로세스가 env를 읽을 수 있는 OS 특성은 오늘의 `BUZZ_PRIVATE_KEY`(영구 비밀)보다 피해가 작다.
- 로그 redaction: `bz[lsrbk]_…` 패턴, `install_report_redaction_tests.rs` 패턴을 따르는 테스트.

### 6.2 Hermes `.env` override 버그 클래스
- Hermes는 `<HERMES_HOME>/.env`를 override=True로 로드한다. 루트 `.env`에 헤르홈의 `bzk_`가 있으면 Buzz 관리 Hermes 에이전트가 그 토큰으로 붙는 것이 재발 형태.
- 방어 1: 프로필별 `HERMES_HOME`(47d3f23d2) + `is_buzz_owned_env_key`의 `BUZZ_*` strip, `BUZZ_BOT_TOKEN` 테스트 명시.
- 방어 2(서버): `bzk_`는 `host_device_id IS NULL`인 봇에만 발급. Desktop 호스팅 봇이 `bzk_`로 AUTH하면 다른 principal이라 "다른 봇"으로 보인다 — 조용한 신원 오염 불가. `host_device_id`가 있는 봇에 `bot_headless` 토큰이 들어오면 거절.
- 방어 3: `reserved_env_keys.rs` 명시 등록.

### 6.3 OIDC 플로우
- PKCE가 "시작한 클라이언트만 완료" 보장. `state`는 Redis 10분 TTL, 1회. `nonce`로 id_token 재생 방지. `login_code`는 60초 1회.
- loopback redirect: 다른 로컬 프로세스가 같은 포트를 선점하는 공격은 PKCE verifier가 없어 `complete`를 못 한다. 리스너는 `127.0.0.1` 바인드, 로그인 동안만.
- `redirect_uri` 허용 목록 외 거절(open redirect 방지). callback은 Google 등록 URI 1개.
- Google client secret은 서버 설정만. 클라이언트 바이너리에 secret 없음.

### 6.4 디바이스 분실
- 다른 디바이스에서 원격 로그아웃 → 세션·access·host 봇 토큰 revoke, WS close. 분실 디바이스의 refresh는 keychain. "모든 다른 기기 로그아웃"으로 일괄.

### 6.5 Exchange / refresh 재전송
- exchange 응답 유실: 서버가 새 토큰을 커밋했지만 응답이 acp에 닿지 않으면 acp는 구 토큰만 가진 채 재시도한다. 두 장치로 막는다: (1) **replay 캐시 `auth:exchange:replay:{old_hash}`(10초)**를 exchange 핸들러의 첫 단계에서 조회해 같은 새 토큰을 재전송한다(§3.3 순서). DB 커밋과 캐시 기록 사이에 도착한 재시도(캐시 miss, `superseded_at`이 10초 이내)는 409 대신 캐시를 50ms×20 폴링해 같은 토큰을 받는다(refresh의 RecentlyRotated와 동일). 캐시 기록은 실패 시 3회까지 재시도한다. (2) 10초를 넘겨 409 `token_superseded`를 받으면 acp는 이를 **auth-terminal(exit 78)**로 분류한다 → Desktop 재발급. 캐시는 새 `bzb_` 평문을 10초 보관한다(refresh 캐시와 같은 비용).
- exchange: 구 토큰 60초 grace. **grace 중(superseded) 토큰은 AUTH와 일반 요청은 가능하지만 exchange는 불가**(`access_tokens.superseded_at IS NOT NULL` → 409 `token_superseded`, 단 위 replay 캐시 조회가 먼저). 따라서 공격자가 grace 안에 구 토큰으로 exchange해 정상 acp의 새 토큰을 revoke시키는 시나리오는 성립하지 않는다. 공격자가 grace 안에 할 수 있는 것은 60초 동안 구 토큰으로 요청하는 것뿐이며, 그 뒤 `exchanged` revoke로 끝난다. 2회/분/봇 제한. (`superseded_at` 컬럼을 §2.2 `access_tokens`에 추가: exchange 트랜잭션이 구 토큰에 `now()`를 기록하고 60초 뒤 태스크가 `revoked_at`을 채운다.)
- refresh: §3.6. 10초 재전송 캐시는 Redis `auth:refresh:replay:{old_hash}`에 **새 access/refresh 평문**을 10초 TTL로 둔다. Redis 침해 시 10초 창의 토큰 쌍이 노출되는 비용을 수용한다(Redis는 이미 세션·revoke 전파의 신뢰 경계 안에 있다). 응답 재전송은 캐시 키가 구 refresh 해시이므로 그 refresh를 가진 자에게만 간다.

### 6.6 Rate limit / 바운드 (`RedisRateLimiter`)
- OIDC start: IP당 30/분. complete: IP당 10/분. 피어 IP를 알 수 없는 리스너(UDS)에서는 IP 버킷 대신 배포 전역 버킷(start 600/분, complete 300/분)을 쓴다. refresh: refresh 토큰당 10/분만(IP 버킷 없음: 프록시 뒤에서 전역 버킷으로 붕괴해 활성 세션 수만으로 한도를 넘는다). **후속(Phase 1+)**: 신뢰 프록시의 `X-Forwarded-For`/`Forwarded` 헤더 지원(신뢰 프록시 CIDR 설정) — 그 전까지 리버스 프록시 뒤 배포는 프록시 IP 하나가 IP 버킷을 공유한다. WS 토큰 re-AUTH는 연결의 `ws_operations` 버킷을 공유한다. exchange: 2/분/봇. 봇 토큰 발급: 10/분/사용자. 디바이스 50/사용자, 봇 100/사용자, 세션당 refresh_tokens row는 generation 100개 초과분을 주기 삭제.
- Desktop 재시작 루프 10분 3회 + 수동 재시작(Rule 4, 6). 연결 만료 타이머 연결당 1개, 종료 시 abort. revoke 채널 수신 실패 시 토큰 `expires_at`(≤1h)로 결국 끊어지고, WS는 10분마다 DB에서 바인딩 토큰 `revoked_at` 재확인.
- 브로커 응답 ≤4KB, 60/분. OIDC 콜백 쿼리 ≤8KB.

### 6.7 Review-Proven Rules 적용

| Rule | 적용 |
|------|------|
| 1 | 봇 삭제는 서버 DELETE 성공 후 로컬 삭제. refresh 실패는 백오프 후 "재로그인 필요" 터미널 상태로 가시화. revoke publish는 커밋 후. |
| 2 | refresh `generation`+`used_at`; acp exchange 세대(`AtomicU64`)로 더 새로운 것만 채택; 재-AUTH 바인딩 교체는 `OK auth true` 수신 후 커밋; Stop/삭제/디바이스 revoke/revoke-all/계정 삭제 모든 경로에서 `access_tokens` 정리 — 경로 열거 테스트. |
| 3 | §5 테스트는 핸들러·스토어·runtime 실제 함수에 바인딩. |
| 4 | §6.6. |
| 5 | 로그인 complete(devices+sessions+refresh_tokens+access_tokens), 봇 생성(principals+bots), exchange(insert+revoke), refresh(used_at+insert+access), 프로필 변경(principals+users projection)은 각각 단일 트랜잭션. |
| 6 | 재시작 상한 후 수동 "다시 시작"/"다시 로그인" 버튼 유지. refresh 영구 실패 시 로그인 화면 경로 보장. |

---

## 7. 확정 사항

| # | 질문 | 결정 | 반영 위치 |
|---|------|------|-----------|
| Q1 | 로그인 방식 | **Google OIDC만**, 비밀번호 없음. `identities(provider, subject)`로 Apple 등 추가 가능. Desktop = 시스템 브라우저+loopback+PKCE, Mobile = 네이티브 세션+PKCE, Web = 같은 Google 로그인 | §2.2, §3.2, §4.2, §4.14–4.17 |
| Q2 | 계정/프로필 범위 | Google 계정 1개 = 계정 1개. 프로필(이름/아바타) **전역**, 커뮤니티별 없음. @username은 선택 | §2.2, §3.7 |
| Q3 | 자식 에이전트 CLI 토큰 | acp loopback 브로커 유지 | §4.11 |
| Q4 | DM | 서버 저장 평문 DM. **DM 모델(`h` 비공개 채널 + 30622)은 무변경**, gift wrap/NIP-44 암호화 레이어만 삭제 | §1.3, §3.8, §4.16 |
| Q5 | 봇 프로필 편집 주체 | owner 세션(`PATCH /auth/bots/{id}/profile`). kind 0은 서버 발행 전용이므로 ingest 예외 없음 | §3.3, §3.5, §3.7 |
| Q6 | `web/` 인증 | 같은 Google 로그인 세션 토큰. **Phase 2 범위** | §4.17, §5.3 |
| Q7 | 기존 에이전트 히스토리 | 단절 수용, claim 마이그레이션 없음 | §4.14, §5.2 |
| Q8 | pair-relay/pairing-cli/mesh | 삭제 | §1.3, §5.5 |
| Q9 | 계정 보안 이벤트 | "모든 다른 기기 로그아웃"은 사람 세션만. 봇 토큰은 "모든 봇 토큰 회수"로 별도. Google 연결 해제 없음(= 계정 삭제, 30일 purge) | §2.4, §2.5 |
| Q10 | 운영자 인증 | principal + Bearer + **기존** `relay_operators`(`AdminRole`, moderator 유지). admin Bearer 분기는 Phase 1(B6), 운영자 관리 엔드포인트·config 우선순위 제거는 Phase 2. 부트스트랩은 매 로그인 평가(B8) | §2.2, §3.3, §4.4, §5.2, §5.3 |
| 일반 | 미결 사항 | KakaoTalk/Telegram이 하는 방식 | 전체 |

---

## 8. v1 → v2 변경 요약

| 블로커/항목 | 변경 |
|-------------|------|
| B1 principal id 유효성 | §2.1 불변식: `Keys::generate().public_key()`로 생성(비밀키 drop), `PrincipalId` 생성자에서 `from_slice` 검증, 스토어 테스트 고정. relay principal은 기동 코드 `ON CONFLICT DO NOTHING` + 부분 유니크 인덱스, SQL seed/reconcile 제거. §3.5에 Phase 4 타입 교체 전까지의 전제로 명시. |
| B2 refresh 재사용 탐지 | `sessions.refresh_token_hash` 삭제 → `refresh_tokens` 테이블(§2.2), rotation 알고리즘 §3.6, 테스트는 프로덕션 핸들러로 `sessions.revoked_at` 단언 + not-found 구분(§5.1). 10초 재전송 완화 추가. |
| B3 exchange가 WS를 끊음 | (a) 채택: acp가 exchange 직후 같은 연결에서 재-AUTH, §3.4에 "재-AUTH는 바인딩 교체"로 명시, `exchanged` revoke는 publish하지 않음(§2.4). WS AUTH 거절 `token_expired|token_revoked|principal_disabled`도 exit 78(§4.11). live 토큰 불변식 "≤2 for 60s, then 1" 명시(§2.3). |
| B4 Phase 3 미해결 의존 | admin/git/audio/bridge/invite/media/workflow/web 토큰화를 Phase 2 범위로 이동(§5.3), `INVITE_SIGNING_SECRET` 도입, Phase 3 삭제 체크리스트 표(§5.4). |
| 사용자 확정 | §7 열린 질문 → 확정 사항. Google OIDC 플로우(§3.2), 전역 프로필(§3.7), 계정 보안 이벤트(§2.5), 운영자 테이블/엔드포인트, web Phase 2. |
| 선택 항목 | `RedisRateLimiter` 재사용; `CITEXT` → `lower()` 인덱스, `handle` → 선택 `username`; `schema.sql` 동시 갱신; 경로 수정(`nip42_host_binding_live.rs` 위치, redaction 테스트 경로); `signRelayEvent` 추가 호출자 4곳; mobile `sig` optional을 Phase 1 선행으로; `executor.rs` env/npub 필터; reserved env 키 4개 명시; `Command::env_remove`; 재시작 상한 후 수동 버튼. |

### v2 → v3

| 블로커/항목 | 변경 |
|-------------|------|
| B5 `relay_operators` 중복 생성 | 신설 테이블 삭제. 기존 테이블(0035, `pubkey` 32바이트 = principal id, `operator\|moderator`)과 `store/relay_operators.rs`·`AdminRole` 재사용. 0056은 ALTER 없음, `added_by`에 relay principal id. `/auth/operators`는 `PUT {role}`로 기존 스토어 호출. config 우선순위 제거는 Phase 2 (§2.2, §3.3, §4.3, §4.4, §5.3). |
| B6 Phase 1에서 에이전트 CLI/git/관리 콘솔 단절 | Phase 1로 이동: buzz-cli 토큰 모드(브로커 소스, Bearer+draft, 키 폴백), `git-credential-buzz` + acp `git.rs:99-100` 브로커 변수, 서버 `authorize_bearer`(admin)·git `transport.rs` Bearer/Basic 분기, Desktop `commands/admin/client.rs` Bearer. §5.3에서 제거, §5.2 테스트·수동 검증 추가, §5.6 갱신 (§4.4, §4.8, §4.12, §4.14, §5.2, §5.3, §5.6). |
| B7 평문 DM이 ingest에서 거절 | DM 모델 무변경(`h` 비공개 채널 + 30622). `requires_h_channel_scope` 유지, 새 p-gate 없음. 암호화 레이어(1059/NIP-44)만 제거. §5.1에 `h` 없는 40002 거절 회귀 가드 (§1.3, §3.8, §7 Q4). |
| B8 운영자 부트스트랩 잠금/오부여 | 조건 = 매 로그인마다 `email_verified==true` AND 이메일 일치 AND operator row 0개 → 1회 부여 + 감사. 로스터 비어 있지 않으면 무시, 전원 삭제 시 재부트스트랩 가능. 테스트 4건 (§3.2, §3.3, §5.1). |
| 선택 항목 | §3.6 replay 캐시 조회를 `used_at` 분기 앞으로; Phase 3 체크리스트에 `relay_keypair` 사용 14파일 추가; `/start?state=` 전달·검증 명시; Google 클라이언트 "Web application" 1개; §3.5 owner→bot kind 0 예외 삭제; web 쿠키(relay same-origin, `SameSite=Strict`, refresh 본문 생략, logout 쿠키 삭제, 별도 origin은 범위 밖); `client=cli` 추가; purge 순서와 `ON DELETE` 정의(`bots.owner_principal_id` 의도적 FK 실패); 동시 첫 로그인 PK 충돌 → 재조회 수렴; `AUTH_TOKEN_ENABLED=false`는 `/auth/*` 전체 404; `remove_var` 근거 수정(edition 2021); exchange는 superseded 토큰 불가(`superseded_at` 컬럼) → 공격자 exchange 시나리오 제거; replay 캐시의 refresh 평문 보관 비용 명시. |

### v3 → v4

| 항목 | 변경 |
|------|------|
| exchange 응답 유실 | `auth:exchange:replay:{old_hash}`(10초) 캐시를 핸들러 첫 단계로, 409 `token_superseded`는 acp auth-terminal(exit 78). 순서 명시 (§3.3, §4.11, §6.5). |
| 운영자 쓰기 경로 | 부트스트랩/`PUT`/`DELETE`는 기존 `upsert`/`remove` 경유(감사 행·advisory lock 유지), `bootstrap_operator`는 `acquire_roster_lock` → count → upsert 한 트랜잭션, `LastOperator` → 409 (§4.3). |
| purge 운영자 제거 | raw DELETE 대신 `remove(actor=relay_principal)`, 마지막 operator면 warn + 감사 `auth.operator_roster_emptied` (§2.2). |
| 부트스트랩 판정 범위 | DB row만 판정(config 운영자 무시), `PUT/DELETE`는 Phase 2까지 `config_operator_exists` 통과 (§3.3). |
| 라인/구조 수정 | `parse_git_auth_header_full`은 `transport.rs:377`; `authorize()`의 `AdminAuth` 모드 분기 바깥에서 Bearer 스킴 분기(또는 `AdminAuth::Token`) (§4.4). |
| Phase 0 구현 주의 | 리스크 3건 박스 추가: sentinel sig 읽기 경로 라이브 테스트, 바인딩 교체(타이머 abort, `BindingKey`=토큰 해시, 재확인 태스크 바운드), 플래그 off 범위 라우터 테스트 + schema.sql/`_operator_global_tables`/pgschema 검증 (§5.1). |

### v4 → v5

| 항목 | 변경 |
|------|------|
| 상태 | Phase 0 구현 완료, 리뷰 READY (2026-10-04). handoff는 §5.1.1. |
| 리뷰 반영 | exchange 커밋 경합 재시도 → `RecentlyExchanged`(DB 시계 10초) + replay 캐시 폴링, 캐시 쓰기 3회 재시도; `/auth/*` per-IP 버킷은 ConnectInfo가 있을 때만, 없으면 배포 전역 버킷(start 600/분, complete 300/분), refresh는 토큰당만; 재-AUTH bind-then-check; 토큰 AUTH 프레임을 `ws_operations` 버킷에 포함; Bearer 스킴 대소문자 무시; refresh 캐시 miss는 503 (§6.5, §6.6). |
| Phase 1 선행 추가 | 신뢰 프록시 forwarding 헤더 (§5.2). |

### v5 → v6

| 항목 | 변경 |
|------|------|
| 상태 | Phase 1 구현 완료(미커밋, 리뷰 전). handoff는 §5.2.1. |
| 추가 결정 | NIP-11 `buzz_token_auth`로 토큰 모드 감지; 커뮤니티별 세션; 공용 `EventSigner`(sentinel draft)와 `verify_served_event`; `buzz-token-broker` 크레이트; `git-credential-buzz` 호스트 고정 + 서버 이중 챌린지; `AUTH_TRUSTED_PROXY_CIDRS`(`unix` 포함). |

### v6 → v7

| 항목 | 변경 |
|------|------|
| 상태 | Phase 1 리뷰 블로커 B1–B6 및 Rule 3 테스트 반영(미커밋). §5.2.1 "리뷰 반영". |
| 결정 변경 | Desktop 별칭 맵 폐기 → 레코드 pubkey를 bot id로 재작성(§4.14 원안). admin Bearer는 Nip98 모드 한정. broker 시크릿은 에이전트 프로세스별 lease. |
| 후속 추가 | client_ip 헤더 선택, sentinel 허용 범위, keyring 실패 마커, 오프라인 로그아웃 재시도, run 모드 refresh, 토큰 모드 kickoff, provider 백엔드. |

### v7 → v8

| 항목 | 변경 |
|------|------|
| 상태 | Phase 1 재리뷰 블로커 N1(이동 후 UI 갱신·옛 pubkey 해석), N2(토큰 모드 welcome을 사용자 이름으로) 및 선택 항목 반영(미커밋). §5.2.1 "재리뷰 반영". |
| 결정 | 로그인 시 키로 실행 중인 에이전트는 자동 재시작 대신 "Restart required". replay 창 상수는 buzz-core 공유. |

### v8 → v9

| 항목 | 변경 |
|------|------|
| 상태 | Phase 1 **READY**(3차 리뷰). closer 사용자 명의 fallback, 지연 자격증명 조회, 삭제 시 대소문자 무시 비교 반영. 미커밋. §5.2.1 "최종 (v9)". |

### v9 → v10

| 항목 | 변경 |
|------|------|
| 상태 | Phase 2(모바일 제외) 구현 완료, 미커밋, 리뷰 전. §5.3.1. 모바일은 Phase 2b로 연기(Flutter 부재). |
| 결정 | relay principal = relay 서명 키(재키잉); relay 키는 Phase 3에서도 유지(S2S 인증), §5.4의 `relay_keypair` 행 무효; kind 0 서버 발행(프로필 쓰기 + 토큰 AUTH reconcile); 운영자 config 우선순위는 플래그 on에서 무시; `INVITE_SIGNING_SECRET` 플래그 on 필수; 초대·오디오·media·web·CLI 토큰화. |

### v10 → v11

| 항목 | 변경 |
|------|------|
| 상태 | Phase 2 리뷰 블로커 B1–B3 및 선택 항목 반영(미커밋). §5.3.1 "리뷰 반영 (v11)". |
| 결정 변경 | 운영자 config는 필드를 지우지 않고 admin 해석 지점만 플래그로 게이트(`RELAY_OWNER_PUBKEY`의 NIP-43·멤버십 필수 용도, `/operator/*` 프로비저닝은 유지); kind 0 발행은 (커뮤니티, principal) advisory lock 아래에서 principal 재조회, AUTH/re-AUTH reconcile은 `users` row가 있는 곳만, 첫 쓰기에서 발행; PATCH fan-out은 스폰(동시 4); 에이전트 프로세스는 `BUZZ_DISABLE_STORED_SESSION=1`. |
