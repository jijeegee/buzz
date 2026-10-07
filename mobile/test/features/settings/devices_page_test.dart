import 'package:buzz/features/settings/devices_page.dart';
import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/devices/device_robot.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../shared/auth/token/token_auth_test_fakes.dart';
import 'token_settings_harness.dart';

Future<TokenSettingsHarness> _pumpDevices(WidgetTester tester) async {
  final h = TokenSettingsHarness();
  h.server.refreshResponses.add(
    (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
  );
  await h.pump(tester, const DevicesPage(origin: tokenOrigin));
  return h;
}

Future<void> _tapAndConfirm(WidgetTester tester, Key key) async {
  if (key == const Key('devices-delete-account')) {
    await tester.ensureVisible(
      find.byKey(const Key('devices-account-deletion')),
    );
    await tester.tap(find.byKey(const Key('devices-account-deletion')));
    await tester.pumpAndSettle();
  }
  await tester.ensureVisible(find.byKey(key));
  await tester.tap(find.byKey(key));
  await frames(tester);
  await tester.tap(find.byKey(const Key('devices-confirm')));
  await frames(tester);
}

void main() {
  testWidgets('account deletion is hidden and cancellation does not delete', (
    tester,
  ) async {
    final h = await _pumpDevices(tester);
    expect(find.byKey(const Key('devices-delete-account')), findsNothing);
    await tester.ensureVisible(
      find.byKey(const Key('devices-account-deletion')),
    );
    await tester.tap(find.byKey(const Key('devices-account-deletion')));
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.byKey(const Key('devices-delete-account')));
    await tester.tap(find.byKey(const Key('devices-delete-account')));
    await frames(tester);
    expect(h.account.calls('DELETE', '/auth/account'), isEmpty);
    await tester.tap(find.text('Cancel'));
    await frames(tester);
    expect(h.account.calls('DELETE', '/auth/account'), isEmpty);
    expect(await h.storage.loadAll(), hasLength(1));
  });

  testWidgets('lists devices and marks this one', (tester) async {
    await _pumpDevices(tester);

    expect(find.text('Test phone'), findsOneWidget);
    expect(find.text('Work laptop'), findsOneWidget);
    expect(find.text('Old browser'), findsOneWidget);
    expect(find.textContaining('This device'), findsOneWidget);
    expect(find.byKey(const Key('device-sign-out-device-1')), findsNothing);
    expect(find.byKey(const Key('device-sign-out-device-2')), findsOneWidget);
  });

  testWidgets('each device row shows its device robot', (tester) async {
    await _pumpDevices(tester);

    for (final id in ['device-1', 'device-2', 'device-3']) {
      final tag = deviceRobotVariantForDevice(id)!.tag;
      expect(
        find.descendant(
          of: find.byKey(Key('device-row-$id')),
          matching: find.byKey(ValueKey('device-robot-$tag')),
        ),
        findsOneWidget,
        reason: id,
      );
    }
  });

  testWidgets('remote sign-out revokes the device and refreshes the list', (
    tester,
  ) async {
    final h = await _pumpDevices(tester);

    await _tapAndConfirm(tester, const Key('device-sign-out-device-2'));

    expect(h.account.calls('DELETE', '/auth/devices/device-2'), hasLength(1));
    expect(h.account.calls('GET', '/auth/devices'), hasLength(2));
    expect(find.text('Work laptop'), findsNothing);
    expect(find.text('Old browser'), findsOneWidget);
  });

  testWidgets('a failed remote sign-out is shown and the list kept', (
    tester,
  ) async {
    final h = await _pumpDevices(tester);
    h.account.failures['/auth/devices/device-2'] = (_) =>
        jsonResponse({'error': 'relay unavailable'}, status: 503);

    await _tapAndConfirm(tester, const Key('device-sign-out-device-2'));

    expect(find.textContaining('relay unavailable'), findsOneWidget);
    expect(find.text('Work laptop'), findsOneWidget);
  });

  testWidgets('sign out all other devices leaves only this one', (
    tester,
  ) async {
    final h = await _pumpDevices(tester);

    await _tapAndConfirm(tester, const Key('devices-sign-out-others'));

    expect(
      h.account.calls('POST', '/auth/sessions/revoke-others'),
      hasLength(1),
    );
    expect(find.text('Work laptop'), findsNothing);
    expect(find.text('Old browser'), findsNothing);
    expect(find.text('Test phone'), findsOneWidget);
  });

  testWidgets('revoke all bot tokens', (tester) async {
    final h = await _pumpDevices(tester);

    await _tapAndConfirm(tester, const Key('devices-revoke-bots'));

    expect(h.account.calls('POST', '/auth/bots/revoke-all'), hasLength(1));
    expect(find.text('All bot tokens revoked'), findsOneWidget);
  });

  testWidgets('cancelling a confirmation sends nothing', (tester) async {
    final h = await _pumpDevices(tester);

    await tester.ensureVisible(find.byKey(const Key('devices-revoke-bots')));
    await tester.tap(find.byKey(const Key('devices-revoke-bots')));
    await frames(tester);
    await tester.tap(find.text('Cancel'));
    await frames(tester);

    expect(h.account.calls('POST', '/auth/bots/revoke-all'), isEmpty);
  });

  testWidgets('delete account: confirm, delete on relay, sign out locally', (
    tester,
  ) async {
    final h = await _pumpDevices(tester);

    await _tapAndConfirm(tester, const Key('devices-delete-account'));

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
    final h = await _pumpDevices(tester);
    h.account.failures['/auth/account'] = (_) =>
        jsonResponse({'error': 'relay unavailable'}, status: 503);

    await _tapAndConfirm(tester, const Key('devices-delete-account'));

    expect(find.textContaining('relay unavailable'), findsOneWidget);
    expect(h.tokens.data[tokenOrigin], isNotNull);
    expect(await h.storage.loadAll(), hasLength(1));
  });

  testWidgets(
    'a deleted account whose local cleanup fails retries only the cleanup',
    (tester) async {
      final h = await _pumpDevices(tester);
      h.tokens.deleteError = StateError('keychain locked');

      await _tapAndConfirm(tester, const Key('devices-delete-account'));

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
