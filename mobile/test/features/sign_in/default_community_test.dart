import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:buzz/features/sign_in/token_sign_in_page.dart';
import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/auth/default_community.dart';
import 'package:buzz/shared/auth/google_key_backup.dart';
import 'package:buzz/shared/auth/token/token.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../shared/auth/google_key_backup_test.dart' as backup;
import '../../shared/auth/token/token_auth_test_fakes.dart';
import '../../shared/community/community_storage_test.dart';
import 'google_key_backup_page_test.dart' show frames;

class _Fixture {
  final server = backup.BackupServer();
  final storage = CommunityStorage(secure: FakeSecureStorage());
  final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
  final admissions = <Community>[];
  final gate = Completer<void>();
  bool reject = false;
  bool wait = false;
  late final ProviderContainer container;

  _Fixture() {
    final auth = FakeAuthServer();
    auth.complete = (_) => jsonResponse({
      'principal_id': backup.account,
      'identity_mode': 'key_backup',
      'device_id': 'phone',
      'access': 'bzs_login',
      'refresh': 'bzr_login',
      'expires_in': 3600,
    });
    final clock = FakeClock(DateTime.now());
    final client = MockClient((request) async {
      final forwarded = http.Request(request.method, request.url)
        ..headers.addAll(request.headers)
        ..bodyBytes = request.bodyBytes;
      return http.Response.fromStream(
        await (request.url.path.startsWith('/auth/key-backup')
                ? server.client
                : auth.client)
            .send(forwarded),
      );
    });
    container = ProviderContainer(
      overrides: [
        defaultCommunityOriginProvider.overrideWithValue(backup.origin),
        authHttpClientProvider.overrideWithValue(client),
        webAuthLauncherProvider.overrideWithValue(launcher),
        sessionClockProvider.overrideWithValue(clock.call),
        sessionTimerFactoryProvider.overrideWithValue(clock.createTimer),
        keyBackupRefreshTokenStoreProvider.overrideWithValue(
          FakeRefreshTokenStore(),
        ),
        pendingBackupKeyStoreProvider.overrideWithValue(
          PendingBackupKeyStore(storage: FakeSecureStorage()),
        ),
        communityStorageProvider.overrideWithValue(storage),
        signedCommunityAdmissionProvider.overrideWithValue((community) async {
          admissions.add(community);
          if (wait) await gate.future;
          if (reject) {
            throw const KeyBackupException('Signed AUTH denied. Retry.');
          }
        }),
      ],
    );
    addTearDown(container.dispose);
  }

  Future<void> show(WidgetTester tester) async {
    await container.read(authProvider.future);
    await tester.pumpWidget(
      UncontrolledProviderScope(
        container: container,
        child: const MaterialApp(home: TokenSignInPage(defaultCommunity: true)),
      ),
    );
    await frames(tester);
  }

  Future<void> login(WidgetTester tester) async {
    await tester.ensureVisible(find.byKey(const Key('token-sign-in-google')));
    await tester.tap(find.byKey(const Key('token-sign-in-google')));
    await frames(tester);
  }
}

void main() {
  for (final returning in [false, true]) {
    testWidgets(
      'default Google entry ${returning ? 'restores' : 'creates on mobile first'} and waits for signed AUTH',
      (tester) async {
        final f = _Fixture()..wait = true;
        final original = nostr.Keys.generate();
        if (returning) f.server.secret = original.secret;
        await f.show(tester);
        expect(find.byKey(const Key('token-sign-in-relay-url')), findsNothing);
        expect(
          find.byKey(const Key('sign-in-custom-community')),
          findsOneWidget,
        );
        await f.login(tester);
        expect(
          f.launcher.opened.single.queryParameters['identity_mode'],
          'key_backup',
        );
        expect(f.admissions, hasLength(1));
        expect(await f.storage.loadAll(), isEmpty);
        expect(
          f.container.read(authProvider).value!.status,
          AuthStatus.unauthenticated,
        );
        f.gate.complete();
        await frames(tester);
        final saved = (await f.storage.loadAll()).single;
        expect(saved.tokenAuth, isFalse);
        expect(saved.pubkey, nostr.Keys(f.server.secret!).public);
        expect(saved.nsec, nostr.Keys(f.server.secret!).nsec);
        expect(saved.pubkey, isNot(backup.account));
        expect(saved.relayUrl, backup.origin);
        expect(
          f.container.read(authProvider).value!.status,
          AuthStatus.authenticated,
        );
        expect(f.server.proofs.length, returning ? 0 : 1);
        if (returning) expect(saved.pubkey, original.public);
      },
    );
  }

  testWidgets(
    'AUTH denial and retry preserve uploaded key and do not duplicate community',
    (tester) async {
      final f = _Fixture()..reject = true;
      await f.show(tester);
      await f.login(tester);
      expect(await f.storage.loadAll(), isEmpty);
      expect(find.text('Signed AUTH denied. Retry.'), findsOneWidget);
      final uploaded = f.server.secret;
      f.reject = false;
      await f.login(tester);
      final saved = (await f.storage.loadAll()).single;
      expect(saved.nsec, nostr.Keys(uploaded!).nsec);
      await f.login(tester);
      expect((await f.storage.loadAll()).single.id, saved.id);
      expect(f.server.proofs, hasLength(1));
    },
  );

  testWidgets('backup read failure cannot enroll or mint a replacement', (
    tester,
  ) async {
    final f = _Fixture();
    f.server.failStatus = true;
    await f.show(tester);
    await f.login(tester);
    expect(f.server.secret, isNull);
    expect(f.server.proofs, isEmpty);
    expect(f.admissions, isEmpty);
    expect(await f.storage.loadAll(), isEmpty);
    expect(find.byKey(const Key('token-sign-in-error')), findsOneWidget);
  });

  testWidgets('leaving during admission cannot persist a late completion', (
    tester,
  ) async {
    final f = _Fixture()..wait = true;
    await f.show(tester);
    await f.login(tester);
    expect(f.admissions, hasLength(1));
    await tester.pumpWidget(const MaterialApp(home: SizedBox()));
    f.gate.complete();
    await frames(tester);
    expect(await f.storage.loadAll(), isEmpty);
    expect(f.server.secret, isNotNull);
  });

  test(
    'configured user restarts with original community selection and key',
    () async {
      final f = _Fixture();
      final key = nostr.Keys.generate();
      final selected = Community.create(
        name: 'Custom',
        relayUrl: 'https://custom.example',
        nsec: key.nsec,
        pubkey: key.public,
      );
      await f.storage.save(
        Community.create(
          name: 'Other',
          relayUrl: backup.origin,
          nsec: nostr.Keys.generate().nsec,
        ),
      );
      await f.storage.save(selected);
      await f.storage.saveActiveId(selected.id);
      final state = await f.container.read(authProvider.future);
      expect(state.community!.id, selected.id);
      expect(state.community!.nsec, key.nsec);
      expect(f.launcher.opened, isEmpty);
      expect(f.admissions, isEmpty);
    },
  );

  testWidgets('selection changed during admission is preserved', (
    tester,
  ) async {
    final f = _Fixture()..wait = true;
    await f.show(tester);
    await f.login(tester);
    final otherKey = nostr.Keys.generate();
    final other = Community.create(
      name: 'Chosen during sign-in',
      relayUrl: 'https://other.example',
      nsec: otherKey.nsec,
      pubkey: otherKey.public,
    );
    await f.container
        .read(authProvider.notifier)
        .authenticateWithCommunity(other);
    f.gate.complete();
    await frames(tester);
    expect((await f.storage.loadAll()).single.id, other.id);
    expect(await f.storage.loadActiveId(), other.id);
  });

  for (final accepted in [false, true]) {
    test(
      'admission uses real signed socket and requires AUTH OK = $accepted',
      () async {
        final key = nostr.Keys.generate();
        final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
        addTearDown(() => server.close(force: true));
        final seen = Completer<nostr.Event>();
        server.listen((request) async {
          final ws = await WebSocketTransformer.upgrade(request);
          addTearDown(ws.close);
          ws.add(jsonEncode(['AUTH', 'test-challenge']));
          ws.listen((raw) {
            final frame = jsonDecode(raw as String) as List;
            if (frame.first != 'AUTH') return;
            final event = nostr.Event.fromMap(frame[1] as Map<String, dynamic>);
            seen.complete(event);
            ws.add(
              jsonEncode([
                'OK',
                event.id,
                accepted,
                accepted ? '' : 'restricted: membership required',
              ]),
            );
          });
        });
        final container = ProviderContainer();
        addTearDown(container.dispose);
        final community = Community.create(
          name: 'Isolated test',
          relayUrl: 'http://127.0.0.1:${server.port}',
          nsec: key.nsec,
          pubkey: key.public,
        );
        final admission = container.read(signedCommunityAdmissionProvider)(
          community,
        );
        if (accepted) {
          await admission;
        } else {
          await expectLater(admission, throwsA(isA<KeyBackupException>()));
        }
        final proof = await seen.future;
        expect(proof.kind, 22242);
        expect(proof.pubkey, key.public);
        expect(
          nostr.Schnorr.verify(
            publicKey: proof.pubkey,
            message: proof.id,
            signature: proof.sig,
          ),
          isTrue,
        );
      },
    );
  }
}
