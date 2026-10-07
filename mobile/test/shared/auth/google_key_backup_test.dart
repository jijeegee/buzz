import 'dart:io';
import 'dart:async';
import 'package:http/http.dart' as http;
import 'dart:convert';

import 'package:buzz/shared/auth/google_key_backup.dart';
import 'package:buzz/shared/auth/auth_provider.dart';
import 'package:buzz/shared/auth/token/token.dart';
import 'package:buzz/shared/community/community.dart';
import 'package:buzz/shared/community/community_provider.dart';
import 'package:buzz/shared/community/community_storage.dart';
import 'package:buzz/shared/relay/relay_provider.dart';
import 'package:buzz/shared/relay/signed_event_relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/testing.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../community/community_storage_test.dart';
import 'token/token_auth_test_fakes.dart';

const origin = 'https://relay.test';
const account = 'account-one';

class BackupServer {
  String? secret;
  bool failStatus = false;
  bool failRestore = false;
  bool failUpload = false;
  String? raceSecret;
  int raceStatus = 409;
  String? restoreSecretOverride;
  final proofs = <nostr.Event>[];

  late final client = MockClient((request) async {
    expect(request.headers['authorization'], 'Bearer bzs_login');
    switch (request.url.path) {
      case '/auth/key-backup':
        if (request.method == 'GET') {
          if (failStatus) return jsonResponse({}, status: 503);
          return jsonResponse({
            'state': secret == null ? 'absent' : 'ready',
            'account_id': account,
            'pubkey': secret == null ? null : nostr.Keys(secret!).public,
            'version': 1,
          });
        }
        final body = jsonDecode(request.body) as Map<String, dynamic>;
        proofs.add(nostr.Event.fromMap(body['proof'] as Map<String, dynamic>));
        if (failUpload) return jsonResponse({}, status: 503);
        if (raceSecret != null) {
          secret = raceSecret;
          return jsonResponse({'code': 'backup_exists'}, status: raceStatus);
        }
        secret = body['secret_key'] as String;
        return jsonResponse({
          'pubkey': nostr.Keys(secret!).public,
          'version': 1,
        });
      case '/auth/key-backup/challenge':
        return jsonResponse({
          'challenge': 'c' * 64,
          'account_id': account,
          'url': '$origin/auth/key-backup',
          'expires_in': 120,
        });
      case '/auth/key-backup/restore':
        if (failRestore) return jsonResponse({}, status: 503);
        return jsonResponse({
          'secret_key': restoreSecretOverride ?? secret,
          'pubkey': nostr.Keys(secret!).public,
          'version': 1,
        });
    }
    return jsonResponse({}, status: 404);
  });
}

Future<TokenSessionController> signedInSession({FakeAuthServer? server}) async {
  final authServer = server ?? FakeAuthServer();
  authServer.complete = (_) => jsonResponse({
    'principal_id': account,
    'identity_mode': 'key_backup',
    'device_id': 'device',
    'access': 'bzs_login',
    'refresh': 'bzr_login',
    'expires_in': 3600,
  });
  final session = TokenSessionController(
    origin: origin,
    api: AuthApi(origin: origin, client: authServer.client),
    store: FakeRefreshTokenStore(),
    launcher: FakeWebAuthLauncher(FakeWebAuthLauncher.success),
  );
  addTearDown(session.dispose);
  expect(await session.signIn(identityMode: 'key_backup'), isTrue);
  return session;
}

void main() {
  test(
    'signed-out account never links its retained key to an absent Google backup',
    () async {
      final keys = nostr.Keys.generate();
      final storage = CommunityStorage(secure: FakeSecureStorage());
      await storage.save(
        Community.create(
          name: 'Saved',
          relayUrl: origin,
          pubkey: keys.public,
          nsec: keys.nsec,
          googleBackupAccountId: account,
        ).copyWith(signedOut: true),
      );
      final server = BackupServer();
      final service = GoogleKeyBackupService(
        origin: origin,
        client: server.client,
        session: await signedInSession(),
        communities: storage,
        pending: PendingBackupKeyStore(storage: FakeSecureStorage()),
        generateKeys: () =>
            throw StateError('Must not generate on reauthentication'),
      );
      await expectLater(service.resolve(), throwsA(isA<KeyBackupException>()));
      expect(server.proofs, isEmpty);
      expect((await storage.loadAll()).single.nsec, keys.nsec);
      server.secret = keys.secret;
      final restored = await service.resolve();
      expect(restored.pubkey, keys.public);
      // Only the guarded auth commit, not restoration itself, clears logout.
      expect((await storage.loadAll()).single.signedOut, isTrue);
    },
  );
  test(
    'mobile-first upload and desktop-first restore share the exact wire key',
    () async {
      // Public secp256k1 test vector shared with Rust resolve_backup tests.
      final wire =
          jsonDecode(
                await File(
                  'test/fixtures/google_key_backup_interop.json',
                ).readAsString(),
              )
              as Map<String, dynamic>;
      final key = nostr.Keys(wire['secret_key'] as String);
      for (final mobileFirst in [true, false]) {
        final server = BackupServer();
        if (!mobileFirst) server.secret = key.secret;
        final result = await GoogleKeyBackupService(
          origin: origin,
          client: server.client,
          session: await signedInSession(),
          communities: CommunityStorage(secure: FakeSecureStorage()),
          pending: PendingBackupKeyStore(storage: FakeSecureStorage()),
          generateKeys: () {
            expect(mobileFirst, isTrue);
            return key;
          },
        ).resolve();
        expect(result.pubkey, wire['pubkey']);
        expect(result.nsec, key.nsec);
        expect(server.secret, wire['secret_key']);
        expect(server.proofs.length, mobileFirst ? 1 : 0);
      }
    },
  );

  for (final cancel in [true, false]) {
    test(
      'stale backup absence is fenced: ${cancel ? "page cancellation" : "same-account new login"}',
      () async {
        final session = await signedInSession();
        final status = Completer<http.Response>();
        final requested = Completer<void>();
        var current = true;
        var generated = 0;
        var requests = 0;
        final service = GoogleKeyBackupService(
          origin: origin,
          client: MockClient((request) {
            requests++;
            requested.complete();
            return status.future;
          }),
          session: session,
          communities: CommunityStorage(secure: FakeSecureStorage()),
          pending: PendingBackupKeyStore(storage: FakeSecureStorage()),
          generateKeys: () {
            generated++;
            return nostr.Keys.generate();
          },
        );
        final resolving = service.resolve(isCurrent: () => current);
        final failed = expectLater(
          resolving,
          throwsA(isA<KeyBackupException>()),
        );
        await requested.future;
        if (cancel) {
          current = false;
        } else {
          await session.signIn(identityMode: 'key_backup');
        }
        status.complete(
          jsonResponse({
            'state': 'absent',
            'account_id': account,
            'pubkey': null,
            'version': 1,
          }),
        );
        await failed;
        expect(generated, 0);
        expect(requests, 1);
      },
    );
  }

  test(
    'missing local key restores only the recorded pubkey from a ready backup',
    () async {
      final keys = nostr.Keys.generate();
      final storage = CommunityStorage(secure: FakeSecureStorage());
      final existing = Community.create(
        name: 'Recover me',
        relayUrl: origin,
        pubkey: keys.public,
        googleBackupAccountId: account,
      );
      await storage.save(existing);
      final server = BackupServer()..secret = keys.secret;
      final service = GoogleKeyBackupService(
        origin: origin,
        client: server.client,
        session: await signedInSession(),
        communities: storage,
        pending: PendingBackupKeyStore(storage: FakeSecureStorage()),
        generateKeys: () => throw StateError('must not generate'),
      );
      final recovered = await service.resolve();
      expect(recovered.id, existing.id);
      expect(recovered.pubkey, existing.pubkey);
      expect(recovered.nsec, keys.nsec);
      expect(recovered.tokenAuth, isFalse);
      server.secret = null;
      await expectLater(service.resolve(), throwsA(isA<KeyBackupException>()));
      server.secret = nostr.Keys.generate().secret;
      await expectLater(service.resolve(), throwsA(isA<KeyBackupException>()));
      expect((await storage.loadAll()).single.nsec, isNull);
      final container = ProviderContainer(
        overrides: [communityStorageProvider.overrideWithValue(storage)],
      );
      addTearDown(container.dispose);
      await container.read(authProvider.future);
      await container
          .read(authProvider.notifier)
          .authenticateWithCommunity(recovered);
      expect((await storage.loadAll()).single.nsec, keys.nsec);
      expect((await storage.loadAll()).single.pubkey, existing.pubkey);
    },
  );
  test(
    'refresh retains custody mode and never changes the signed identity',
    () async {
      final auth = FakeAuthServer();
      final session = await signedInSession(server: auth);
      final server = BackupServer();
      final community = await GoogleKeyBackupService(
        origin: origin,
        client: server.client,
        session: session,
        communities: CommunityStorage(secure: FakeSecureStorage()),
        pending: PendingBackupKeyStore(storage: FakeSecureStorage()),
      ).resolve();
      final config = RelayConfig(baseUrl: origin, nsec: community.nsec);
      auth.refreshResponses.add(
        (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
      );
      await session.refreshAfterTokenExpired('bzs_login');
      expect(session.identityMode, 'key_backup');
      expect(session.state.principalId, account);
      expect(outgoingAuthorPubkey(config), community.pubkey);
      expect(config.tokenAuth, isFalse);
    },
  );

  test(
    'wrong recovered key and conflicting local identity fail closed',
    () async {
      final local = nostr.Keys.generate();
      final remote = nostr.Keys.generate();
      final server = BackupServer()..secret = remote.secret;
      final storage = CommunityStorage(secure: FakeSecureStorage());
      final session = await signedInSession();
      final service = GoogleKeyBackupService(
        origin: origin,
        client: server.client,
        session: session,
        communities: storage,
        pending: PendingBackupKeyStore(storage: FakeSecureStorage()),
        generateKeys: () => throw StateError('must not generate'),
      );
      server.restoreSecretOverride = local.secret;
      await expectLater(service.resolve(), throwsA(isA<KeyBackupException>()));
      server.restoreSecretOverride = '0' * 64;
      await expectLater(service.resolve(), throwsA(isA<KeyBackupException>()));
      expect(await storage.loadAll(), isEmpty);
      server.restoreSecretOverride = null;
      final existing = Community.create(
        name: 'Original',
        relayUrl: origin,
        nsec: local.nsec,
        pubkey: local.public,
      );
      await storage.save(existing);
      await expectLater(service.resolve(), throwsA(isA<KeyBackupException>()));
      expect((await storage.loadAll()).single.nsec, local.nsec);
    },
  );
  test(
    'signup backs up client key; second device restores same signed author',
    () async {
      final server = BackupServer();
      final session = await signedInSession();
      var generated = 0;
      GoogleKeyBackupService device() => GoogleKeyBackupService(
        origin: origin,
        client: server.client,
        session: session,
        communities: CommunityStorage(secure: FakeSecureStorage()),
        pending: PendingBackupKeyStore(storage: FakeSecureStorage()),
        generateKeys: () {
          generated++;
          return nostr.Keys.generate();
        },
      );
      final first = await device().resolve();
      final second = await device().resolve();
      expect(generated, 1);
      expect(first.pubkey, second.pubkey);
      expect(first.nsec, second.nsec);
      expect(first.tokenAuth, isFalse);
      final event = buildOutgoingEvent(
        RelayConfig(baseUrl: origin, nsec: second.nsec),
        kind: 40002,
        content: 'hello',
        tags: [
          ['p', first.pubkey!],
        ],
      );
      expect(event.pubkey, first.pubkey);
      expect(
        nostr.Schnorr.verify(
          publicKey: event.pubkey,
          message: event.id,
          signature: event.sig!,
        ),
        isTrue,
      );
      expect(
        server.proofs.single.tags.map((tag) => tag.join(':')),
        contains('account:$account'),
      );
      expect(
        server.proofs.single.tags.map((tag) => tag.join(':')),
        contains('action:initialize'),
      );
    },
  );

  test('existing key linking preserves community and signing key', () async {
    final keys = nostr.Keys.generate();
    final storage = CommunityStorage(secure: FakeSecureStorage());
    final existing = Community.create(
      name: 'Existing',
      relayUrl: origin,
      pubkey: keys.public,
      nsec: keys.nsec,
    );
    await storage.save(existing);
    final server = BackupServer();
    final result = await GoogleKeyBackupService(
      origin: origin,
      client: server.client,
      session: await signedInSession(),
      communities: storage,
      pending: PendingBackupKeyStore(storage: FakeSecureStorage()),
      generateKeys: () => throw StateError('must not generate'),
    ).resolve();
    expect(result.id, existing.id);
    expect(result.nsec, existing.nsec);
    expect(server.secret, keys.secret);
  });

  test('status and restore failures never generate a replacement', () async {
    final server = BackupServer()..failStatus = true;
    var generated = 0;
    final service = GoogleKeyBackupService(
      origin: origin,
      client: server.client,
      session: await signedInSession(),
      communities: CommunityStorage(secure: FakeSecureStorage()),
      pending: PendingBackupKeyStore(storage: FakeSecureStorage()),
      generateKeys: () {
        generated++;
        return nostr.Keys.generate();
      },
    );
    await expectLater(service.resolve(), throwsA(isA<KeyBackupException>()));
    server
      ..failStatus = false
      ..secret = nostr.Keys.generate().secret
      ..failRestore = true;
    await expectLater(service.resolve(), throwsA(isA<KeyBackupException>()));
    expect(generated, 0);
  });

  test(
    'uncertain upload retries durable pending key without regenerating',
    () async {
      final server = BackupServer()..failUpload = true;
      final secure = FakeSecureStorage();
      final session = await signedInSession();
      var generated = 0;
      GoogleKeyBackupService service() => GoogleKeyBackupService(
        origin: origin,
        client: server.client,
        session: session,
        communities: CommunityStorage(secure: FakeSecureStorage()),
        pending: PendingBackupKeyStore(storage: secure),
        generateKeys: () {
          generated++;
          return nostr.Keys.generate();
        },
      );
      await expectLater(
        service().resolve(),
        throwsA(isA<KeyBackupException>()),
      );
      server.failUpload = false;
      final result = await service().resolve();
      expect(generated, 1);
      expect(server.proofs.first.pubkey, result.pubkey);
    },
  );

  test(
    'a backup_exists body on HTTP 503 cannot authorize conflict recovery',
    () async {
      final server = BackupServer()
        ..raceSecret = nostr.Keys.generate().secret
        ..raceStatus = 503;
      final storage = CommunityStorage(secure: FakeSecureStorage());
      final service = GoogleKeyBackupService(
        origin: origin,
        client: server.client,
        session: await signedInSession(),
        communities: storage,
        pending: PendingBackupKeyStore(storage: FakeSecureStorage()),
      );
      await expectLater(service.resolve(), throwsA(isA<KeyBackupException>()));
      expect(await storage.loadAll(), isEmpty);
    },
  );

  test(
    'concurrent initialization restores winner only on new device',
    () async {
      final server = BackupServer()..raceSecret = nostr.Keys.generate().secret;
      final result = await GoogleKeyBackupService(
        origin: origin,
        client: server.client,
        session: await signedInSession(),
        communities: CommunityStorage(secure: FakeSecureStorage()),
        pending: PendingBackupKeyStore(storage: FakeSecureStorage()),
      ).resolve();
      expect(result.pubkey, nostr.Keys(server.raceSecret!).public);
    },
  );

  test(
    'existing token community and different linked key remain untouched',
    () async {
      final storage = CommunityStorage(secure: FakeSecureStorage());
      final existing = Community.create(
        name: 'Token data',
        relayUrl: origin,
        pubkey: 'token-principal',
        tokenAuth: true,
      );
      await storage.save(existing);
      final server = BackupServer();
      final service = GoogleKeyBackupService(
        origin: origin,
        client: server.client,
        session: await signedInSession(),
        communities: storage,
        pending: PendingBackupKeyStore(storage: FakeSecureStorage()),
        generateKeys: () => throw StateError('must not generate'),
      );
      await expectLater(service.resolve(), throwsA(isA<KeyBackupException>()));
      expect((await storage.loadAll()).single.toJson(), existing.toJson());
    },
  );
}
