import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel_head_cache.dart';
import 'package:kuzz/shared/auth/auth.dart';
import 'package:kuzz/shared/auth/google_key_backup.dart';
import 'package:kuzz/shared/auth/token/token.dart';
import 'package:kuzz/shared/community/community_token_session.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/message_store_provider.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../community/community_storage_test.dart';
import 'token/token_auth_test_fakes.dart';

const _origin = 'https://relay.test';

class _RecordingStore extends Fake implements MessageStore {
  final forgotten = <String>[];

  @override
  Future<void> forget(String relayUrl, String account) async {
    forgotten.add(account);
  }
}

class _RecordingHeads extends Fake implements ChannelHeadCache {
  final cleared = <String>[];

  @override
  Future<void> clearAccount(ChannelHeadScope scope) async {
    cleared.add(scope.pubkey);
  }
}

class _Harness {
  final storage = CommunityStorage(secure: FakeSecureStorage());
  final tokens = FakeRefreshTokenStore();
  final messages = _RecordingStore();
  final heads = _RecordingHeads();
  final ended = <String>[];
  final journaled = <String>[];
  final pending = PendingBackupKeyStore(storage: FakeSecureStorage());
  Object? enderError;
  late final ProviderContainer container;

  AuthNotifier get auth => container.read(authProvider.notifier);

  Future<void> start(List<Community> rows, {String? activeId}) async {
    await storage.saveAll(rows);
    await storage.saveActiveId(activeId ?? rows.first.id);
    container = ProviderContainer(
      overrides: [
        communityStorageProvider.overrideWithValue(storage),
        communitySnapshotWriterProvider.overrideWithValue((_) async {}),
        refreshTokenStoreProvider.overrideWithValue(tokens),
        keyBackupRefreshTokenStoreProvider.overrideWithValue(tokens),
        authHttpClientProvider.overrideWithValue(FakeAuthServer().client),
        messageStoreProvider.overrideWithValue(messages),
        channelHeadCacheProvider.overrideWithValue(heads),
        pendingBackupKeyStoreProvider.overrideWithValue(pending),
        communityPushLeaseRevocationEnqueuerProvider.overrideWithValue((
          community,
        ) async {
          journaled.add(community.id);
          return false;
        }),
        communityLogoutSessionEnderProvider.overrideWithValue((
          community, {
          required deviceOnly,
        }) async {
          if (enderError case final error?) throw error;
          ended.add(community.id);
        }),
      ],
    );
    addTearDown(container.dispose);
    await container.read(authProvider.future);
    await container.read(communityListProvider.future);
  }

  Future<Community> row(String id) async =>
      (await storage.loadAll()).singleWhere((c) => c.id == id);
}

Community _google(String account, nostr.Keys keys, {DateTime? lastUsedAt}) =>
    Community.create(
      name: account,
      relayUrl: _origin,
      pubkey: keys.public,
      nsec: keys.nsec,
      googleBackupAccountId: account,
    ).copyWith(lastUsedAt: lastUsedAt);

void main() {
  test('logout deletes the pending key of that account only', () async {
    final pending = PendingBackupKeyStore(storage: FakeSecureStorage());
    final container = ProviderContainer(
      overrides: [
        keyBackupRefreshTokenStoreProvider.overrideWithValue(
          FakeRefreshTokenStore(),
        ),
        authHttpClientProvider.overrideWithValue(FakeAuthServer().client),
        pendingBackupKeyStoreProvider.overrideWithValue(pending),
      ],
    );
    addTearDown(container.dispose);
    final keys = nostr.Keys.generate();
    await pending.write(_origin, 'account-a', keys.nsec);
    await pending.write(_origin, 'account-b', keys.nsec);

    await container.read(communityLogoutSessionEnderProvider)(
      _google('account-a', keys),
      deviceOnly: true,
    );

    expect(await pending.read(_origin, 'account-a'), isNull);
    expect(await pending.read(_origin, 'account-b'), isNotNull);
  });

  final monday = DateTime.utc(2026, 11, 2);
  final tuesday = DateTime.utc(2026, 11, 3);
  final wednesday = DateTime.utc(2026, 11, 4);

  group('one server, two accounts', () {
    late nostr.Keys keysA;
    late nostr.Keys keysB;
    late Community a;
    late Community b;

    setUp(() {
      keysA = nostr.Keys.generate();
      keysB = nostr.Keys.generate();
      a = _google('account-a', keysA);
      b = _google('account-b', keysB);
    });

    test('sign-out ends the active account and goes to the other', () async {
      final h = _Harness();
      await h.start([a, b]);

      await h.auth.signOut();

      expect(h.ended, [a.id]);
      expect(await h.storage.loadActiveId(), b.id);
      final state = h.container.read(authProvider).value;
      expect(state?.status, AuthStatus.authenticated);
      expect(state?.community?.id, b.id);
      final savedA = await h.row(a.id);
      expect(savedA.signedOut, isTrue);
      expect(savedA.nsec, isNull);
      final savedB = await h.row(b.id);
      expect(savedB.signedOut, isFalse);
      expect(savedB.nsec, keysB.nsec);
      expect(h.messages.forgotten, [keysA.public]);
      expect(h.heads.cleared, [keysA.public]);
    });

    test('an inactive account signed out elsewhere goes alone', () async {
      final h = _Harness();
      await h.start([a, b]);
      var transitions = 0;
      h.container.read(communityTransitionProvider).register(() async {
        transitions++;
      });

      await h.auth.signOutCommunity(b.id, deviceOnly: true);

      expect(transitions, 0, reason: 'a huddle of A keeps going');
      expect((await h.row(b.id)).signedOut, isTrue);
      expect((await h.row(a.id)).signedOut, isFalse);
      expect((await h.row(a.id)).nsec, keysA.nsec);
      expect(await h.storage.loadActiveId(), a.id);
      expect(h.container.read(authProvider).value?.community?.id, a.id);
      expect(h.messages.forgotten, [keysB.public]);
      expect(h.heads.cleared, [keysB.public]);
    });

    test('sign-out ends the account on screen when it is called', () async {
      final h = _Harness();
      await h.start([a, b]);
      final hold = Completer<void>();
      final queue = h.container.read(communityTransitionProvider);
      unawaited(queue.runExclusive(() => hold.future));

      // A notification tap ahead of the sign-out moves the app to B first.
      final switching = h.container
          .read(communityListProvider.notifier)
          .switchCommunity(b.id);
      final signingOut = h.auth.signOut();
      hold.complete();
      await Future.wait([switching, signingOut]);

      expect((await h.row(a.id)).signedOut, isTrue);
      expect((await h.row(b.id)).signedOut, isFalse);
      expect(await h.storage.loadActiveId(), b.id);
    });

    test('an offline sign-out leaves nothing journaled', () async {
      final h = _Harness();
      await h.start([a, b]);
      h.enderError = const TokenSessionException('offline');

      await expectLater(
        h.auth.signOut(),
        throwsA(isA<TokenSessionException>()),
      );

      expect(h.journaled, isEmpty);
      expect((await h.row(a.id)).signedOut, isFalse);
    });

    test('a fallback with no key goes to its sign-in page', () async {
      final keyless = Community.create(
        name: 'Keyless',
        relayUrl: _origin,
        pubkey: keysB.public,
        googleBackupAccountId: 'account-b',
      );
      final h = _Harness();
      await h.start([a, keyless]);

      await h.auth.signOut();

      final state = h.container.read(authProvider).value;
      expect(state?.status, AuthStatus.unauthenticated);
      expect(state?.community?.id, keyless.id);
    });

    test('a fallback that can never sign in is dropped, as at start', () async {
      final unusable = Community.create(
        name: 'Unusable',
        relayUrl: _origin,
        pubkey: keysB.public,
      );
      final h = _Harness();
      await h.start([a, unusable]);

      await h.auth.signOut();

      expect((await h.storage.loadAll()).map((row) => row.id), [a.id]);
      expect(
        h.container.read(authProvider).value?.status,
        AuthStatus.unauthenticated,
      );
    });

    test('a key the row holds leaves no pending copy behind', () async {
      final h = _Harness();
      await h.start([a]);
      await h.pending.write(_origin, 'account-b', keysB.nsec);

      await h.auth.authenticateWithCommunity(b);

      expect(await h.pending.read(_origin, 'account-b'), isNull);
    });

    test('the last account signed out shows the signed-out page', () async {
      final h = _Harness();
      await h.start([a, b]);

      await h.auth.signOut();
      await h.auth.signOut();

      final state = h.container.read(authProvider).value;
      expect(state?.status, AuthStatus.unauthenticated);
      expect(state?.community?.id, b.id);
      expect(await h.storage.loadAll(), hasLength(2));
    });

    test('sign-out goes to the most recent signed-in account', () async {
      final older = _google('older', nostr.Keys.generate(), lastUsedAt: monday);
      final recent = _google(
        'recent',
        nostr.Keys.generate(),
        lastUsedAt: tuesday,
      );
      final gone = _google(
        'gone',
        nostr.Keys.generate(),
        lastUsedAt: wednesday,
      ).copyWith(signedOut: true);
      final h = _Harness();
      await h.start([a, older, gone, recent]);

      await h.auth.signOut();

      expect(await h.storage.loadActiveId(), recent.id);
    });

    test('a sign-in of a new account leaves the row of A', () async {
      final h = _Harness();
      await h.start([a]);
      final before = (await h.row(a.id)).toJson();
      final c = _google('account-c', nostr.Keys.generate());

      await h.auth.authenticateWithCommunity(c);

      expect((await h.row(a.id)).toJson(), before);
      expect(await h.storage.loadActiveId(), c.id);
      expect(h.messages.forgotten, isEmpty);
      expect(h.heads.cleared, isEmpty);
      expect(h.journaled, isEmpty);
    });

    test('token account B signs in: A keeps row, token and messages', () async {
      final tokenA = Community.create(
        name: 'A',
        relayUrl: _origin,
        pubkey: 'a' * 64,
        tokenAuth: true,
      );
      final stored = StoredTokenSession(
        refreshToken: 'bzr_a',
        principalId: 'a' * 64,
      );
      final h = _Harness();
      h.tokens.data[SessionKey(_origin, 'a' * 64)] = stored;
      await h.start([tokenA]);
      final before = (await h.row(tokenA.id)).toJson();

      await h.auth.authenticateWithTokenSession(
        relayUrl: _origin,
        principalId: 'b' * 64,
      );

      final rows = await h.storage.loadAll();
      expect(rows, hasLength(2));
      expect((await h.row(tokenA.id)).toJson(), before);
      expect(h.tokens.data[SessionKey(_origin, 'a' * 64)], stored);
      expect(h.messages.forgotten, isEmpty);
      final added = rows.singleWhere((row) => row.id != tokenA.id);
      expect(await h.storage.loadActiveId(), added.id);
      expect(added.pubkey, 'b' * 64);
    });

    test('the same account with its paired key drops the old key', () async {
      final h = _Harness();
      await h.start([a, b]);
      final paired = nostr.Keys.generate();

      await h.auth.authenticateWithCommunity(
        a.withCredentials(pubkey: paired.public, nsec: paired.nsec),
      );

      final saved = await h.row(a.id);
      expect(saved.pubkey, paired.public);
      expect(saved.nsec, paired.nsec);
      expect(saved.googleBackupAccountId, 'account-a');
      expect(h.journaled, [a.id]);
      expect(h.messages.forgotten, [keysA.public]);
      expect(h.heads.cleared, [keysA.public]);
      expect((await h.row(b.id)).nsec, keysB.nsec);
    });

    test('a signed-out account coming back keeps its row', () async {
      final h = _Harness();
      await h.start([a, b]);
      await h.auth.signOut();
      final signedOut = await h.row(a.id);

      await h.auth.authenticateWithCommunity(
        signedOut.withCredentials(pubkey: keysA.public, nsec: keysA.nsec),
      );

      expect((await h.storage.loadAll()), hasLength(2));
      final saved = await h.row(a.id);
      expect(saved.signedOut, isFalse);
      expect(saved.nsec, keysA.nsec);
      expect(await h.storage.loadActiveId(), a.id);
    });

    test("one account's sign-out does not stop another's sign-in", () async {
      final h = _Harness();
      await h.start([a, b]);
      final transitions = h.container.read(communityTransitionProvider);
      final block = Completer<void>();
      final entered = Completer<void>();
      final pending = transitions.runExclusive(() async {
        entered.complete();
        await block.future;
      });
      await entered.future;
      final c = _google('account-c', nostr.Keys.generate());
      final signIn = h.auth.authenticateWithCommunity(c);
      final signOut = h.auth.signOutCommunity(b.id, deviceOnly: true);
      block.complete();
      await Future.wait([pending, signIn, signOut]);

      expect((await h.row(b.id)).signedOut, isTrue);
      expect((await h.row(c.id)).signedOut, isFalse);
      expect(await h.storage.loadActiveId(), c.id);
    });

    test('a sign-out does stop a sign-in of the same account', () async {
      final h = _Harness();
      await h.start([a, b]);
      final transitions = h.container.read(communityTransitionProvider);
      final block = Completer<void>();
      final entered = Completer<void>();
      final pending = transitions.runExclusive(() async {
        entered.complete();
        await block.future;
      });
      await entered.future;
      final signIn = h.auth.authenticateWithCommunity(b);
      final rejected = expectLater(signIn, throwsStateError);
      final signOut = h.auth.signOutCommunity(b.id, deviceOnly: true);
      block.complete();
      await Future.wait([pending, rejected, signOut]);

      expect((await h.row(b.id)).signedOut, isTrue);
    });
  });

  group('last used', () {
    test('sign-in and switch record it', () async {
      final a = _google('account-a', nostr.Keys.generate());
      final b = _google('account-b', nostr.Keys.generate());
      final h = _Harness();
      await h.start([a, b]);
      expect((await h.row(b.id)).lastUsedAt, isNull);

      await h.container
          .read(communityListProvider.notifier)
          .switchCommunity(b.id);
      expect((await h.row(b.id)).lastUsedAt, isNotNull);

      await h.auth.authenticateWithCommunity(await h.row(a.id));
      expect((await h.row(a.id)).lastUsedAt, isNotNull);
    });

    test('survives storage', () {
      final community = _google(
        'account-a',
        nostr.Keys.generate(),
        lastUsedAt: tuesday,
      );
      final copy = Community.fromJson(community.toJson());
      expect(copy.lastUsedAt, tuesday);
      expect(
        Community.fromJson(
          _google('account-b', nostr.Keys.generate()).toJson(),
        ).lastUsedAt,
        isNull,
      );
    });

    test('the selector skips signed-out rows', () {
      final oldest = _google('a', nostr.Keys.generate(), lastUsedAt: monday);
      final newer = _google('b', nostr.Keys.generate(), lastUsedAt: tuesday);
      final signedOut = _google(
        'c',
        nostr.Keys.generate(),
        lastUsedAt: wednesday,
      ).copyWith(signedOut: true);
      final never = _google('d', nostr.Keys.generate());

      expect(
        mostRecentlyUsedCommunity([never, oldest, signedOut, newer]),
        newer,
      );
      expect(mostRecentlyUsedCommunity([never]), never);
      expect(mostRecentlyUsedCommunity([signedOut]), isNull);
    });

    test('the stored active account wins only while signed in', () {
      final newest = _google('a', nostr.Keys.generate(), lastUsedAt: tuesday);
      final stored = _google('b', nostr.Keys.generate(), lastUsedAt: monday);
      final rows = [stored, newest];

      expect(
        selectActiveCommunity(rows, stored.id, builtInOrigin: null)?.id,
        stored.id,
      );
      expect(
        selectActiveCommunity(rows, null, builtInOrigin: null)?.id,
        newest.id,
      );
      final out = stored.copyWith(signedOut: true);
      expect(
        selectActiveCommunity([out, newest], out.id, builtInOrigin: null)?.id,
        newest.id,
      );
      final allOut = [out, newest.copyWith(signedOut: true)];
      expect(
        selectActiveCommunity(allOut, out.id, builtInOrigin: null)?.id,
        out.id,
      );
    });
  });

  group('finding the row of an account', () {
    final key = nostr.Keys.generate();
    final other = nostr.Keys.generate();
    final linked = _google('account-a', key);
    final keyOnly = Community.create(
      name: 'Key only',
      relayUrl: _origin,
      pubkey: other.public,
      nsec: other.nsec,
    );
    final token = Community.create(
      name: 'Token',
      relayUrl: _origin,
      pubkey: 't' * 64,
      tokenAuth: true,
    );
    final rows = [linked, keyOnly, token];

    test('a Google account finds only its own row', () {
      expect(
        findKeyAccountRow(rows, origin: _origin, account: 'account-a'),
        linked,
      );
      expect(
        findKeyAccountRow(rows, origin: _origin, account: 'account-z'),
        isNull,
      );
    });

    test('a key-only row is found only by the key paired on the server', () {
      expect(
        findKeyAccountRow(
          rows,
          origin: _origin,
          account: 'account-z',
          pubkey: other.public,
        ),
        keyOnly,
      );
      expect(
        findKeyAccountRow(
          rows,
          origin: _origin,
          account: 'account-z',
          pubkey: key.public,
        ),
        isNull,
        reason: 'that key belongs to another Google account',
      );
    });

    test('a token account finds its own row or none', () {
      expect(
        findTokenAccountRow(rows, origin: _origin, principal: 't' * 64),
        token,
      );
      expect(
        findTokenAccountRow(rows, origin: _origin, principal: 'u' * 64),
        isNull,
      );
      expect(
        findTokenAccountRow(rows, origin: _origin, principal: key.public),
        isNull,
        reason: 'a Google row answers to its Google account',
      );
    });

    test('another server is never matched', () {
      expect(
        findKeyAccountRow(
          rows,
          origin: 'https://other.test',
          account: 'account-a',
        ),
        isNull,
      );
    });
  });
}
