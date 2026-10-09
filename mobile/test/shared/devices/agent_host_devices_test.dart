import 'dart:convert';

import 'package:buzz/shared/devices/agent_badge_widgets.dart';
import 'package:buzz/shared/devices/agent_host_devices.dart';
import 'package:buzz/shared/devices/device_robot.dart';
import 'package:buzz/shared/devices/device_robot_icon.dart';
import 'package:buzz/shared/profile/user_cache_provider.dart';
import 'package:buzz/shared/profile/user_profile.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'own_device_events_fake.dart';

final _owner = 'a' * 64;
final _agentA = '1' * 64;
final _agentB = '2' * 64;
final _agentC = '3' * 64;

var _ids = 0;

NostrEvent _hostEvent(
  String device,
  List<Object?> agents, {
  int createdAt = 100,
  String? pubkey,
  String? id,
  Object? content,
}) => NostrEvent(
  id: id ?? (_ids++).toRadixString(16).padLeft(64, '0'),
  pubkey: pubkey ?? _owner,
  createdAt: createdAt,
  kind: kindAgentHostDevices,
  tags: [
    ['d', device],
  ],
  content: content is String
      ? content
      : jsonEncode(content ?? {'v': 1, 'agents': agents}),
);

NostrEvent _robotEvent(
  String device,
  Object? content, {
  int createdAt = 100,
  String? pubkey,
}) => NostrEvent(
  id: (_ids++).toRadixString(16).padLeft(64, '0'),
  pubkey: pubkey ?? _owner,
  createdAt: createdAt,
  kind: kindDeviceRobot,
  tags: [
    ['d', device],
  ],
  content: content is String ? content : jsonEncode(content),
);

class _FakeUserCache extends UserCacheNotifier {
  @override
  Map<String, UserProfile> build() => const {};

  @override
  UserProfile? get(String pubkey) => state[pubkey];
}

void main() {
  group('agentHostDevicesFromEvents', () {
    test('the newest event listing an agent decides its device', () {
      final map = agentHostDevicesFromEvents([
        _hostEvent('DEVICE-1', [_agentA, _agentB], createdAt: 100),
        _hostEvent('device-2', [_agentB.toUpperCase()], createdAt: 200),
        _hostEvent('device-3', [_agentC], createdAt: 50),
      ], owner: _owner);

      expect(map, {
        _agentA: 'device-1',
        _agentB: 'device-2',
        _agentC: 'device-3',
      });
    });

    test('only the newest event per device counts', () {
      final map = agentHostDevicesFromEvents([
        _hostEvent('device-1', [_agentA], createdAt: 300),
        _hostEvent('device-1', [_agentB], createdAt: 400),
        _hostEvent('device-2', [_agentA], createdAt: 200),
      ], owner: _owner);

      // device-1 no longer lists agent A; device-2 still does.
      expect(map, {_agentA: 'device-2', _agentB: 'device-1'});
    });

    test('malformed events and other authors are ignored', () {
      final map = agentHostDevicesFromEvents([
        _hostEvent('device-1', [_agentA]),
        _hostEvent('device-2', [_agentB], createdAt: 900, pubkey: 'b' * 64),
        _hostEvent('', [_agentB], createdAt: 900),
        _hostEvent('device-3', const [], content: 'not json', createdAt: 900),
        _hostEvent('device-4', const [], content: {'v': 2, 'agents': []}),
        _hostEvent('device-5', ['not-a-pubkey']),
        _hostEvent('device-6', [7]),
        _hostEvent('device-7', const [], content: {'v': 1}),
      ], owner: _owner);

      expect(map, {_agentA: 'device-1'});
    });
  });

  group('deviceRobotOverridesFromEvents', () {
    test('newest choice per device wins; bad choices fall back', () {
      final map = deviceRobotOverridesFromEvents([
        _robotEvent('device-1', {'v': 1, 'shape': 'hex', 'color': 0}),
        _robotEvent('DEVICE-1', {
          'v': 1,
          'shape': 'dome',
          'color': 7,
        }, createdAt: 200),
        _robotEvent('device-2', {'v': 1, 'shape': 'blob', 'color': 1}),
        _robotEvent('device-3', {'v': 1, 'shape': 'tall', 'color': 8}),
        _robotEvent('device-4', {'v': 1, 'shape': 'boxy', 'color': 2}),
        // A newer bad choice resets device-4 to its default.
        _robotEvent('device-4', 'nope', createdAt: 200),
        _robotEvent('device-5', {
          'v': 1,
          'shape': 'visor',
          'color': 3,
        }, pubkey: 'b' * 64),
      ], owner: _owner);

      expect(map.keys, ['device-1']);
      expect(map['device-1']!.shape, 'dome');
      expect(map['device-1']!.colorIndex, 7);
    });

    test('choice content round-trips', () {
      final robot = deviceRobotVariantFromIndices(
        colorIndex: 4,
        shapeIndex: 2,
      )!;
      expect(jsonDecode(deviceRobotChoiceContent(robot)), {
        'v': 1,
        'shape': 'boxy',
        'color': 4,
      });
      expect(parseDeviceRobotChoice(deviceRobotChoiceContent(robot)), robot);
    });
  });

  group('own device events provider', () {
    late FakeOwnDeviceSession session;
    late ProviderContainer container;

    setUp(() {
      session = FakeOwnDeviceSession();
      container = ProviderContainer(
        overrides: [
          relaySessionProvider.overrideWith(() => session),
          relayConfigProvider.overrideWith(() => FixedTokenRelayConfig(_owner)),
        ],
      );
      addTearDown(container.dispose);
    });

    test('subscribes to both kinds and maps live events', () async {
      container.listen(agentHostDevicesProvider, (_, _) {});
      await Future<void>.delayed(Duration.zero);

      expect(session.filters, hasLength(1));
      expect(session.filters.single.kinds, [
        kindAgentHostDevices,
        kindDeviceRobot,
      ]);
      expect(session.filters.single.authors, [_owner]);

      session.emit(_hostEvent('device-1', [_agentA]));
      session.emit(
        _robotEvent('device-1', {'v': 1, 'shape': 'tall', 'color': 5}),
      );
      expect(container.read(agentHostDevicesProvider), {_agentA: 'device-1'});
      expect(
        container.read(deviceRobotOverridesProvider)['device-1']!.shape,
        'tall',
      );
    });

    test(
      'publishing a robot sends kind:30181 and applies it at once',
      () async {
        container.listen(deviceRobotOverridesProvider, (_, _) {});
        await Future<void>.delayed(Duration.zero);
        final robot = deviceRobotVariantFromIndices(
          colorIndex: 1,
          shapeIndex: 5,
        )!;

        await container
            .read(ownDeviceEventsProvider.notifier)
            .publishDeviceRobot('Device-1', robot);

        final event = session.published.single;
        expect(event.kind, kindDeviceRobot);
        expect(event.pubkey, _owner);
        expect(event.tags, [
          ['d', 'device-1'],
        ]);
        expect(jsonDecode(event.content), {'v': 1, 'shape': 'hex', 'color': 1});
        expect(container.read(deviceRobotOverridesProvider), {
          'device-1': robot,
        });
      },
    );
  });

  group('agent robot widgets', () {
    final chosen = deviceRobotVariantFromIndices(colorIndex: 2, shapeIndex: 1)!;

    Future<void> pump(WidgetTester tester, Widget child) {
      return tester.pumpWidget(
        ProviderScope(
          overrides: [
            userCacheProvider.overrideWith(_FakeUserCache.new),
            agentHostDevicesProvider.overrideWithValue({_agentA: 'device-1'}),
            deviceRobotOverridesProvider.overrideWithValue({
              'device-1': chosen,
            }),
            agentHostDeviceNamesProvider.overrideWithValue(const {
              'device-1': 'Studio PC',
            }),
          ],
          child: MaterialApp(home: Center(child: child)),
        ),
      );
    }

    testWidgets('the owner sees the chosen device robot and its name', (
      tester,
    ) async {
      await pump(
        tester,
        AgentRobotGlyph(
          agentPubkey: _agentA,
          ownerPubkey: _owner,
          viewerPubkey: _owner,
        ),
      );

      expect(
        tester
            .widget<DeviceRobotIcon>(
              find.byKey(ValueKey('agent-device-robot-$_agentA')),
            )
            .variant,
        chosen,
      );
      expect(
        tester.widget<Tooltip>(find.byType(Tooltip)).message,
        'Running on Studio PC',
      );
    });

    testWidgets('another viewer sees the default bot glyph', (tester) async {
      await pump(
        tester,
        AgentRobotGlyph(
          agentPubkey: _agentA,
          ownerPubkey: _owner,
          viewerPubkey: 'b' * 64,
        ),
      );

      expect(find.byKey(ValueKey('agent-default-robot-$_agentA')), findsOne);
      expect(find.byType(Tooltip), findsNothing);
    });

    testWidgets('an owner agent without a host device keeps the default', (
      tester,
    ) async {
      await pump(
        tester,
        AgentRobotGlyph(
          agentPubkey: _agentB,
          ownerPubkey: _owner,
          viewerPubkey: _owner,
        ),
      );

      expect(find.byKey(ValueKey('agent-default-robot-$_agentB')), findsOne);
    });
  });
}
