import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:kuzz/features/sign_in/token_sign_in_page.dart';
import 'package:kuzz/shared/auth/default_community.dart';
import 'package:kuzz/shared/auth/auth.dart';
import 'package:kuzz/shared/auth/google_key_backup.dart';
import 'package:kuzz/shared/auth/token/token.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../shared/auth/google_key_backup_test.dart' as backup;
import '../../shared/auth/token/token_auth_test_fakes.dart';
import '../../shared/community/community_storage_test.dart';

const _email = 'new.person@example.com';

Map<String, dynamic> _grant() => {
  'principal_id': backup.account,
  'identity_mode': 'key_backup',
  'device_id': 'device',
  'access': 'bzs_login',
  'refresh': 'bzr_login',
  'expires_in': 3600,
};

class _Harness {
  final storage = CommunityStorage(secure: FakeSecureStorage());
  final server = backup.BackupServer();
  final auth = FakeAuthServer();
  final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
  final tokens = FakeRefreshTokenStore();
  final signups = <Map<String, dynamic>>[];

  /// The account does not exist yet.
  bool needsSignup = true;

  /// What the relay knows of the new account. Apple may withhold it.
  String? signupEmail = _email;

  /// Answers to `signup` before it succeeds.
  final signupFailures = <http.Response>[];
  late final ProviderContainer container;

  Future<void> open(
    WidgetTester tester, {
    TokenSignInPage page = const TokenSignInPage(backupOrigin: backup.origin),
  }) async {
    auth.nip11 = (_) => jsonResponse({
      'kuzz_key_backup': {
        'version': 1,
        'providers': ['google'],
      },
    });
    auth.complete = (_) => needsSignup
        ? jsonResponse({
            'needs_signup': true,
            'signup_token': 'bzn_ticket',
            'email': signupEmail,
          })
        : jsonResponse(_grant());
    final client = MockClient((request) async {
      if (request.url.path == '/auth/oidc/signup') {
        signups.add(jsonDecode(request.body) as Map<String, dynamic>);
        if (signupFailures.isNotEmpty) return signupFailures.removeAt(0);
        return jsonResponse(_grant());
      }
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
    final clock = FakeClock(DateTime.now());
    container = ProviderContainer(
      overrides: [
        authHttpClientProvider.overrideWithValue(client),
        webAuthLauncherProvider.overrideWithValue(launcher),
        sessionClockProvider.overrideWithValue(clock.call),
        sessionTimerFactoryProvider.overrideWithValue(clock.createTimer),
        installIdStoreProvider.overrideWithValue(
          InstallIdStore(storage: FakeSecureStorage()),
        ),
        keyBackupRefreshTokenStoreProvider.overrideWithValue(tokens),
        pendingBackupKeyStoreProvider.overrideWithValue(
          PendingBackupKeyStore(storage: FakeSecureStorage()),
        ),
        communityStorageProvider.overrideWithValue(storage),
        communitySnapshotWriterProvider.overrideWithValue((_) async {}),
        signedCommunityAdmissionProvider.overrideWithValue((_) async {}),
      ],
    );
    addTearDown(container.dispose);
    await container.read(authProvider.future);
    await tester.pumpWidget(
      UncontrolledProviderScope(
        container: container,
        child: MaterialApp(home: page),
      ),
    );
    await frames(tester);
  }

  Future<void> tapGoogle(WidgetTester tester) async {
    final google = find.byKey(const Key('token-sign-in-google'));
    await tester.ensureVisible(google);
    await tester.tap(google);
    await frames(tester);
  }

  Future<void> expectNothingLeft() async {
    expect(await storage.loadAll(), isEmpty);
    expect(signups, isEmpty);
    expect(server.proofs, isEmpty);
    expect(server.secret, isNull);
    expect(tokens.data, isEmpty);
    expect(
      container.read(authProvider).value?.status,
      AuthStatus.unauthenticated,
    );
  }
}

Future<void> frames(WidgetTester tester) async {
  for (var i = 0; i < 30; i++) {
    await tester.pump(const Duration(milliseconds: 16));
  }
}

void main() {
  testWidgets('a new account is asked about, showing the chosen email', (
    tester,
  ) async {
    final h = _Harness();
    await h.open(tester);
    await h.tapGoogle(tester);

    expect(h.launcher.opened.single.queryParameters['signup'], 'confirm');
    expect(find.text('Create account'), findsWidgets);
    expect(find.text(_email), findsOneWidget);
    expect(find.text('Use a different account'), findsOneWidget);
    expect(find.textContaining('key'), findsNothing);
    expect(find.textContaining('server'), findsNothing);
    await h.expectNothingLeft();
  });

  testWidgets('leaving the question leaves nothing', (tester) async {
    final h = _Harness();
    await h.open(tester);
    await h.tapGoogle(tester);

    await tester.binding.handlePopRoute();
    await frames(tester);

    expect(find.byKey(const Key('create-account')), findsNothing);
    expect(find.byKey(const Key('token-sign-in-google')), findsOneWidget);
    await h.expectNothingLeft();
  });

  testWidgets('Create account creates exactly one key and one row', (
    tester,
  ) async {
    final h = _Harness();
    await h.open(tester);
    await h.tapGoogle(tester);

    await tester.tap(find.byKey(const Key('create-account-confirm')));
    await frames(tester);

    expect(h.signups, hasLength(1));
    expect(h.signups.single['signup_token'], 'bzn_ticket');
    expect(h.signups.single['code_verifier'], isA<String>());
    expect(h.server.proofs, hasLength(1));
    final row = (await h.storage.loadAll()).single;
    expect(row.pubkey, nostr.Keys(h.server.secret!).public);
    expect(row.googleBackupAccountId, backup.account);
    expect(
      h.container.read(authProvider).value?.status,
      AuthStatus.authenticated,
    );
  });

  testWidgets('Use a different account signs in again', (tester) async {
    final h = _Harness();
    await h.open(tester);
    await h.tapGoogle(tester);
    final existing = nostr.Keys.generate();
    h.server.secret = existing.secret;
    h.needsSignup = false;

    await tester.tap(find.byKey(const Key('create-account-different')));
    await frames(tester);

    expect(h.launcher.opened, hasLength(2));
    expect(h.signups, isEmpty);
    expect(h.server.proofs, isEmpty);
    expect((await h.storage.loadAll()).single.pubkey, existing.public);
  });

  testWidgets('an existing account is not asked', (tester) async {
    final h = _Harness()..needsSignup = false;
    h.server.secret = nostr.Keys.generate().secret;
    await h.open(tester);
    await h.tapGoogle(tester);

    expect(find.byKey(const Key('create-account')), findsNothing);
    expect((await h.storage.loadAll()), hasLength(1));
  });

  testWidgets('a stale proof at creation does not ask again', (tester) async {
    final h = _Harness();
    h.signupFailures.add(FakeAuthServer.authError(401, 'device_proof_stale'));
    await h.open(tester);
    await h.tapGoogle(tester);

    await tester.tap(find.byKey(const Key('create-account-confirm')));
    await frames(tester);

    expect(find.byKey(const Key('create-account')), findsNothing);
    expect(h.signups, hasLength(2));
    expect(h.server.proofs, hasLength(1));
    expect(await h.storage.loadAll(), hasLength(1));
  });

  testWidgets('with no email a new sign-in always asks', (tester) async {
    final h = _Harness()..signupEmail = null;
    h.signupFailures.add(FakeAuthServer.authError(401, 'device_proof_stale'));
    await h.open(tester);
    await h.tapGoogle(tester);

    await tester.tap(find.byKey(const Key('create-account-confirm')));
    await frames(tester);

    // Nothing shows the second sign-in is the same account.
    expect(find.byKey(const Key('create-account')), findsOneWidget);
    expect(h.signups, hasLength(1));

    await tester.tap(find.byKey(const Key('create-account-confirm')));
    await frames(tester);

    expect(find.byKey(const Key('create-account')), findsNothing);
    expect(h.signups, hasLength(2));
  });

  testWidgets('Add account asks about a new account as well', (tester) async {
    final h = _Harness();
    final other = Community.create(
      name: 'Other',
      relayUrl: backup.origin,
      pubkey: 'o' * 64,
      tokenAuth: true,
    );
    await h.storage.save(other);
    await h.storage.saveActiveId(other.id);
    await h.open(
      tester,
      page: const TokenSignInPage(
        backupOrigin: backup.origin,
        addAccount: true,
      ),
    );
    await h.tapGoogle(tester);
    expect(find.byKey(const Key('create-account')), findsOneWidget);

    await tester.tap(find.byKey(const Key('create-account-confirm')));
    await frames(tester);

    expect(await h.storage.loadAll(), hasLength(2));
    expect(
      h.container.read(authProvider).value?.community?.googleBackupAccountId,
      backup.account,
    );
  });

  testWidgets('Add account of a key account already here only switches', (
    tester,
  ) async {
    final h = _Harness()..needsSignup = false;
    final keys = nostr.Keys.generate();
    final here = Community.create(
      name: 'Here',
      relayUrl: backup.origin,
      pubkey: keys.public,
      nsec: keys.nsec,
      googleBackupAccountId: backup.account,
    );
    final other = Community.create(
      name: 'Other',
      relayUrl: backup.origin,
      pubkey: 'o' * 64,
      tokenAuth: true,
    );
    await h.storage.save(here);
    await h.storage.save(other);
    await h.storage.saveActiveId(other.id);
    await h.open(
      tester,
      page: const TokenSignInPage(
        backupOrigin: backup.origin,
        addAccount: true,
      ),
    );
    await h.tapGoogle(tester);

    expect(await h.storage.loadActiveId(), here.id);
    expect(await h.storage.loadAll(), hasLength(2));
    expect(h.server.proofs, isEmpty, reason: 'nothing touched the key backup');
  });

  testWidgets('Connect Google does not ask', (tester) async {
    final keys = nostr.Keys.generate();
    final h = _Harness()..needsSignup = false;
    final legacy = Community.create(
      name: 'Saved',
      relayUrl: backup.origin,
      pubkey: keys.public,
      nsec: keys.nsec,
    );
    await h.storage.save(legacy);
    await h.storage.saveActiveId(legacy.id);
    await h.open(
      tester,
      page: const TokenSignInPage(
        backupOrigin: backup.origin,
        connectGoogle: true,
      ),
    );
    await h.tapGoogle(tester);

    expect(h.launcher.opened.single.queryParameters, isNot(contains('signup')));
    expect(find.byKey(const Key('create-account')), findsNothing);
    expect(h.server.secret, keys.secret);
    expect(
      (await h.storage.loadAll()).single.googleBackupAccountId,
      backup.account,
    );
  });
}
