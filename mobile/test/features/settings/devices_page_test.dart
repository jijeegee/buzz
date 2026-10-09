import 'dart:convert';

import 'package:buzz/features/settings/devices_page.dart';
import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/devices/agent_host_devices.dart';
import 'package:buzz/shared/devices/device_robot.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../shared/auth/token/token_auth_test_fakes.dart';
import '../../shared/devices/own_device_events_fake.dart';
import 'token_settings_harness.dart';

late FakeOwnDeviceSession _session;

Future<TokenSettingsHarness> _pumpDevices(WidgetTester tester) async {
  final h = TokenSettingsHarness();
  h.server.refreshResponses.add(
    (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
  );
  _session = FakeOwnDeviceSession();
  await h.pump(
    tester,
    const DevicesPage(origin: tokenOrigin),
    overrides: [relaySessionProvider.overrideWith(() => _session)],
  );
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

  testWidgets('picking a robot publishes it and shows it at once', (
    tester,
  ) async {
    final h = await _pumpDevices(tester);

    await tester.tap(find.byKey(const Key('device-robot-device-2')));
    await frames(tester);
    // Every shape and colour is offered as the robot it gives.
    for (final shape in deviceRobotShapes) {
      expect(find.byKey(Key('device-robot-shape-$shape')), findsOneWidget);
    }
    for (var color = 0; color < deviceRobotColors.length; color++) {
      expect(find.byKey(Key('device-robot-color-$color')), findsOneWidget);
    }
    await tester.tap(find.byKey(const Key('device-robot-shape-visor')));
    await tester.tap(find.byKey(const Key('device-robot-color-5')));
    await tester.pump();
    await tester.tap(find.byKey(const Key('device-robot-save')));
    await frames(tester);

    final event = _session.published.single;
    expect(event.kind, kindDeviceRobot);
    expect(event.tags, [
      ['d', 'device-2'],
    ]);
    expect(jsonDecode(event.content), {'v': 1, 'shape': 'visor', 'color': 5});
    final chosen = deviceRobotVariantFromIndices(
      colorIndex: 5,
      shapeIndex: deviceRobotShapes.indexOf('visor'),
    )!;
    expect(h.container.read(deviceRobotOverridesProvider), {
      'device-2': chosen,
    });
    expect(
      find.descendant(
        of: find.byKey(const Key('device-row-device-2')),
        matching: find.byKey(ValueKey('device-robot-${chosen.tag}')),
      ),
      findsOneWidget,
    );
  });

  testWidgets('cancelling the robot picker publishes nothing', (tester) async {
    await _pumpDevices(tester);

    await tester.tap(find.byKey(const Key('device-robot-device-1')));
    await frames(tester);
    await tester.tap(find.byKey(const Key('device-robot-color-3')));
    await tester.tap(find.text('Cancel'));
    await frames(tester);

    expect(_session.published, isEmpty);
  });

  testWidgets('renaming a device refreshes the names agents show', (
    tester,
  ) async {
    final h = await _pumpDevices(tester);
    h.container.listen(agentHostDeviceNamesProvider, (_, _) {});
    await frames(tester);
    expect(
      h.container.read(agentHostDeviceNamesProvider)['device-1'],
      'Test phone',
    );

    await tester.tap(find.byKey(const Key('device-rename-device-1')));
    await frames(tester);
    await tester.enterText(
      find.byKey(const Key('device-rename-field')),
      'Pocket phone',
    );
    await tester.tap(find.byKey(const Key('device-rename-save')));
    await frames(tester);

    expect(
      h.container.read(agentHostDeviceNamesProvider)['device-1'],
      'Pocket phone',
    );
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

  testWidgets('renaming a device saves the trimmed name and refreshes', (
    tester,
  ) async {
    final h = await _pumpDevices(tester);

    await tester.tap(find.byKey(const Key('device-rename-device-1')));
    await frames(tester);
    await tester.enterText(
      find.byKey(const Key('device-rename-field')),
      '  Pocket phone  ',
    );
    await tester.tap(find.byKey(const Key('device-rename-save')));
    await frames(tester);

    final patches = h.account.calls('PATCH', '/auth/devices/device-1');
    expect(patches, hasLength(1));
    expect(patches.single.body, '{"name":"Pocket phone"}');
    expect(find.text('Pocket phone'), findsOneWidget);
    expect(find.text('Test phone'), findsNothing);
  });

  testWidgets('cancelling or clearing a rename sends nothing', (tester) async {
    final h = await _pumpDevices(tester);

    await tester.tap(find.byKey(const Key('device-rename-device-2')));
    await frames(tester);
    await tester.tap(find.text('Cancel'));
    await frames(tester);
    await tester.tap(find.byKey(const Key('device-rename-device-2')));
    await frames(tester);
    await tester.enterText(find.byKey(const Key('device-rename-field')), '  ');
    await tester.tap(find.byKey(const Key('device-rename-save')));
    await frames(tester);

    expect(h.account.calls('PATCH', '/auth/devices/device-2'), isEmpty);
    expect(find.text('Work laptop'), findsOneWidget);
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
