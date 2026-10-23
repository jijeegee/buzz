import 'dart:convert';

import 'package:kuzz/features/settings/devices_page.dart';
import 'package:kuzz/shared/devices/agent_host_devices.dart';
import 'package:kuzz/shared/devices/device_robot.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../shared/auth/token/token_auth_test_fakes.dart';
import '../../shared/devices/own_device_events_fake.dart';
import 'token_settings_harness.dart';

late FakeOwnDeviceSession _session;

Future<TokenSettingsHarness> _pumpDevices(
  WidgetTester tester, {
  bool agents = true,
}) async {
  final h = TokenSettingsHarness();
  h.server.refreshResponses.add(
    (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
  );
  _session = FakeOwnDeviceSession();
  await h.pump(
    tester,
    DevicesPage(origin: tokenOrigin, agents: agents),
    overrides: [relaySessionProvider.overrideWith(() => _session)],
  );
  return h;
}

Future<void> _tapAndConfirm(WidgetTester tester, Key key) async {
  await tester.ensureVisible(find.byKey(key));
  await tester.tap(find.byKey(key));
  await frames(tester);
  await tester.tap(find.byKey(const Key('devices-confirm')));
  await frames(tester);
}

void main() {
  testWidgets('Devices & security: devices and agents, not deletion', (
    tester,
  ) async {
    await _pumpDevices(tester);
    expect(find.text('Devices & security'), findsOneWidget);
    expect(find.text('Test phone'), findsOneWidget);
    expect(find.byKey(const Key('devices-sign-out-others')), findsOneWidget);
    expect(find.byKey(const Key('devices-revoke-bots')), findsOneWidget);
    expect(find.byKey(const Key('devices-account-deletion')), findsNothing);
  });

  testWidgets('a Google key account is not offered disconnecting agents', (
    tester,
  ) async {
    await _pumpDevices(tester, agents: false);
    expect(find.byKey(const Key('devices-sign-out-others')), findsOneWidget);
    expect(find.byKey(const Key('devices-revoke-bots')), findsNothing);
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
    expect(jsonDecode(event.content), {'v': 2, 'shape': 'visor', 'color': 5});
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
}
