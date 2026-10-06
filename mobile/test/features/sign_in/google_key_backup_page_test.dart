import 'package:buzz/shared/auth/default_community.dart';
import 'package:buzz/features/sign_in/token_sign_in_page.dart';
import 'package:buzz/shared/auth/auth.dart';
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

class RecordingAuth extends AuthNotifier {
  Community? saved;
  @override
  Future<AuthState> build() async =>
      const AuthState(status: AuthStatus.unauthenticated);
  @override
  Future<void> authenticateWithCommunity(
    Community community, {
    bool Function()? isCurrent,
  }) async => saved = community;
  @override
  Future<void> authenticateWithTokenSession({
    required String relayUrl,
    required String principalId,
  }) => throw StateError('Google custody must never select token messaging');
}

class ReturningTokenAuth extends RecordingAuth {
  String? tokenPrincipal;
  @override
  Future<void> authenticateWithTokenSession({
    required String relayUrl,
    required String principalId,
  }) async {
    tokenPrincipal = principalId;
  }
}

Future<void> frames(WidgetTester tester) async {
  for (var i = 0; i < 30; i++) {
    await tester.pump(const Duration(milliseconds: 16));
  }
}

void main() {
  for (final existingBackup in [false, true]) {
    testWidgets(
      'dual-mode return to existing token account; local backed key = $existingBackup',
      (tester) async {
        final server = FakeAuthServer();
        server.nip11 = (_) => jsonResponse({
          'buzz_key_backup': {
            'version': 1,
            'providers': ['google'],
          },
          'buzz_token_auth': {
            'bearer': true,
            'oidc_providers': ['google'],
          },
        });
        final storage = CommunityStorage(secure: FakeSecureStorage());
        final keys = nostr.Keys.generate();
        if (existingBackup) {
          await storage.save(
            Community.create(
              name: 'Original',
              relayUrl: backup.origin,
              pubkey: keys.public,
              nsec: keys.nsec,
              googleBackupAccountId: 'backup-account',
            ),
          );
        }
        final auth = ReturningTokenAuth();
        final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
        final clock = FakeClock(DateTime.now());
        final backupStore = FakeRefreshTokenStore();
        final container = ProviderContainer(
          overrides: [
            authHttpClientProvider.overrideWithValue(server.client),
            webAuthLauncherProvider.overrideWithValue(launcher),
            sessionClockProvider.overrideWithValue(clock.call),
            sessionTimerFactoryProvider.overrideWithValue(clock.createTimer),
            refreshTokenStoreProvider.overrideWithValue(
              FakeRefreshTokenStore(),
            ),
            keyBackupRefreshTokenStoreProvider.overrideWithValue(backupStore),
            communityStorageProvider.overrideWithValue(storage),
            authProvider.overrideWith(() => auth),
            signedCommunityAdmissionProvider.overrideWithValue((_) async {}),
          ],
        );
        addTearDown(container.dispose);
        await tester.pumpWidget(
          UncontrolledProviderScope(
            container: container,
            child: const MaterialApp(home: TokenSignInPage()),
          ),
        );
        await tester.enterText(
          find.byKey(const Key('token-sign-in-relay-url')),
          backup.origin,
        );
        await tester.tap(find.byKey(const Key('token-sign-in-check')));
        await frames(tester);
        final returning = find.byKey(
          const Key('token-sign-in-existing-account'),
        );
        expect(returning, findsOneWidget);
        await tester.ensureVisible(returning);
        await tester.tap(returning);
        await frames(tester);
        if (existingBackup) {
          expect(find.textContaining('cannot replace'), findsOneWidget);
          expect(launcher.opened, isEmpty);
          expect(auth.tokenPrincipal, isNull);
          expect((await storage.loadAll()).single.nsec, keys.nsec);
        } else {
          await tester.ensureVisible(
            find.byKey(const Key('token-sign-in-google')),
          );
          await tester.tap(find.byKey(const Key('token-sign-in-google')));
          await frames(tester);
          expect(
            launcher.opened.single.queryParameters['identity_mode'],
            'token',
          );
          expect(auth.tokenPrincipal, 'p' * 64);
          expect(auth.saved, isNull);
          expect(backupStore.data, isEmpty);
          expect(
            server.requests.where(
              (r) => r.url.path.startsWith('/auth/key-backup'),
            ),
            isEmpty,
          );
        }
      },
    );
  }
  testWidgets(
    'Google button creates signed community and uses custody OIDC mode',
    (tester) async {
      final authServer = FakeAuthServer();
      authServer.nip11 = (_) => jsonResponse({
        'buzz_key_backup': {
          'version': 1,
          'providers': ['google'],
        },
        'buzz_token_auth': {
          'bearer': true,
          'oidc_providers': ['google'],
        },
      });
      authServer.complete = (_) => jsonResponse({
        'principal_id': backup.account,
        'identity_mode': 'key_backup',
        'device_id': 'device',
        'access': 'bzs_login',
        'refresh': 'bzr_login',
        'expires_in': 3600,
      });
      final server = backup.BackupServer();
      final client = MockClient((request) async {
        final forwarded = http.Request(request.method, request.url)
          ..headers.addAll(request.headers)
          ..bodyBytes = request.bodyBytes;
        return http.Response.fromStream(
          await (request.url.path.startsWith('/auth/key-backup')
                  ? server.client
                  : authServer.client)
              .send(forwarded),
        );
      });
      final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
      final auth = RecordingAuth();
      final clock = FakeClock(DateTime.now());
      final tokenStore = FakeRefreshTokenStore();
      tokenStore.data[backup.origin] = const StoredTokenSession(
        refreshToken: 'keep-old-token',
        principalId: 'old-token-account',
      );
      final container = ProviderContainer(
        overrides: [
          authHttpClientProvider.overrideWithValue(client),
          webAuthLauncherProvider.overrideWithValue(launcher),
          sessionClockProvider.overrideWithValue(clock.call),
          sessionTimerFactoryProvider.overrideWithValue(clock.createTimer),
          refreshTokenStoreProvider.overrideWithValue(tokenStore),
          keyBackupRefreshTokenStoreProvider.overrideWithValue(
            FakeRefreshTokenStore(),
          ),
          pendingBackupKeyStoreProvider.overrideWithValue(
            PendingBackupKeyStore(storage: FakeSecureStorage()),
          ),
          communityStorageProvider.overrideWithValue(
            CommunityStorage(secure: FakeSecureStorage()),
          ),
          authProvider.overrideWith(() => auth),
          signedCommunityAdmissionProvider.overrideWithValue((_) async {}),
        ],
      );
      addTearDown(container.dispose);
      await container.read(authProvider.future);
      await tester.pumpWidget(
        UncontrolledProviderScope(
          container: container,
          child: const MaterialApp(home: TokenSignInPage()),
        ),
      );
      await tester.enterText(
        find.byKey(const Key('token-sign-in-relay-url')),
        backup.origin,
      );
      await tester.tap(find.byKey(const Key('token-sign-in-check')));
      await frames(tester);
      expect(
        find.textContaining('operator or anyone controlling'),
        findsOneWidget,
        reason: tester
            .widgetList<Text>(find.byType(Text))
            .map((text) => text.data)
            .join(' | '),
      );
      await tester.ensureVisible(find.byKey(const Key('token-sign-in-google')));
      await tester.tap(find.byKey(const Key('token-sign-in-google')));
      await frames(tester);
      expect(
        launcher.opened.single.queryParameters['identity_mode'],
        'key_backup',
      );
      expect(auth.saved, isNotNull);
      expect(auth.saved!.tokenAuth, isFalse);
      expect(auth.saved!.nsec, isNotNull);
      expect(auth.saved!.pubkey, isNot(backup.account));
      expect(tokenStore.data[backup.origin]!.refreshToken, 'keep-old-token');
    },
  );
}
