import 'package:kuzz/features/settings/account_actions.dart';
import 'package:kuzz/shared/auth/auth.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../shared/auth/token/token_auth_test_fakes.dart';
import 'token_settings_harness.dart';

Future<TokenSettingsHarness> _pumpActions(WidgetTester tester) async {
  final h = TokenSettingsHarness();
  h.server.refreshResponses.add(
    (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
  );
  await h.pump(
    tester,
    Scaffold(
      body: ListView(
        children: const [
          DisconnectAgentsRow(origin: tokenOrigin),
          AccountDeletionSection(origin: tokenOrigin),
        ],
      ),
    ),
  );
  return h;
}

/// Opens the folded section and walks both deletion steps; [word] is what
/// gets typed in the second.
Future<void> _deleteAccount(
  WidgetTester tester, {
  String word = accountDeletionConfirmWord,
}) async {
  await tester.ensureVisible(find.byKey(const Key('devices-account-deletion')));
  await tester.tap(find.byKey(const Key('devices-account-deletion')));
  await tester.pumpAndSettle();
  await tester.ensureVisible(find.byKey(const Key('devices-delete-account')));
  await tester.tap(find.byKey(const Key('devices-delete-account')));
  await frames(tester);
  await tester.tap(find.byKey(const Key('devices-confirm')));
  await frames(tester);
  await tester.enterText(
    find.byKey(const Key('account-delete-confirm-field')),
    word,
  );
  await frames(tester);
  await tester.tap(find.byKey(const Key('account-delete-confirm')));
  await frames(tester);
}

void main() {
  testWidgets('disconnect all agents revokes every agent token', (
    tester,
  ) async {
    final h = await _pumpActions(tester);

    await tester.tap(find.byKey(const Key('devices-revoke-bots')));
    await frames(tester);
    await tester.tap(find.byKey(const Key('devices-confirm')));
    await frames(tester);

    expect(h.account.calls('POST', '/auth/bots/revoke-all'), hasLength(1));
    expect(find.text('All agents were disconnected.'), findsOneWidget);
  });

  testWidgets('cancelling disconnect sends nothing', (tester) async {
    final h = await _pumpActions(tester);

    await tester.tap(find.byKey(const Key('devices-revoke-bots')));
    await frames(tester);
    await tester.tap(find.text('Cancel'));
    await frames(tester);

    expect(h.account.calls('POST', '/auth/bots/revoke-all'), isEmpty);
  });

  testWidgets('account deletion is folded and cancelling deletes nothing', (
    tester,
  ) async {
    final h = await _pumpActions(tester);
    expect(find.byKey(const Key('devices-delete-account')), findsNothing);

    await tester.tap(find.byKey(const Key('devices-account-deletion')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('devices-delete-account')));
    await frames(tester);
    await tester.tap(find.text('Cancel'));
    await frames(tester);

    expect(h.account.calls('DELETE', '/auth/account'), isEmpty);
    expect(await h.storage.loadAll(), hasLength(1));
  });

  testWidgets('the second step stays locked until "delete" is typed', (
    tester,
  ) async {
    final h = await _pumpActions(tester);

    await _deleteAccount(tester, word: 'delet');

    final button = tester.widget<FilledButton>(
      find.byKey(const Key('account-delete-confirm')),
    );
    expect(button.onPressed, isNull);
    expect(h.account.calls('DELETE', '/auth/account'), isEmpty);
  });

  testWidgets('delete account: two steps, delete on relay, sign out locally', (
    tester,
  ) async {
    final h = await _pumpActions(tester);

    await _deleteAccount(tester);

    expect(h.account.calls('DELETE', '/auth/account'), hasLength(1));
    // The relay already ended the session: no separate logout.
    expect(
      h.server.requests.where((r) => r.url.path == '/auth/logout'),
      isEmpty,
    );
    expect(h.tokens.data, isEmpty);
    expect(await h.storage.loadAll(), isEmpty);
    expect(
      h.container.read(authProvider).value?.status,
      AuthStatus.unauthenticated,
    );
  });

  testWidgets('a failed delete keeps the account and the community', (
    tester,
  ) async {
    final h = await _pumpActions(tester);
    h.account.failures['/auth/account'] = (_) =>
        jsonResponse({'error': 'relay unavailable'}, status: 503);

    await _deleteAccount(tester);

    expect(find.textContaining('relay unavailable'), findsOneWidget);
    expect(h.tokens.data[tokenOrigin], isNotNull);
    expect(await h.storage.loadAll(), hasLength(1));
  });

  testWidgets(
    'a deleted account whose local cleanup fails retries only the cleanup',
    (tester) async {
      final h = await _pumpActions(tester);
      h.tokens.deleteError = StateError('keychain locked');

      await _deleteAccount(tester);

      expect(h.account.calls('DELETE', '/auth/account'), hasLength(1));
      expect(
        find.byKey(const Key('devices-local-cleanup-error')),
        findsOneWidget,
      );
      expect(find.textContaining('account was deleted'), findsOneWidget);
      // The server side is done: deleting again is not offered.
      expect(find.byKey(const Key('devices-delete-account')), findsNothing);

      h.tokens.deleteError = null;
      await tester.tap(find.byKey(const Key('devices-local-cleanup-retry')));
      await frames(tester);

      expect(h.account.calls('DELETE', '/auth/account'), hasLength(1));
      expect(h.tokens.data, isEmpty);
      expect(await h.storage.loadAll(), isEmpty);
    },
  );
}
