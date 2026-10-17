import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/agents/my_agents.dart';
import 'package:kuzz/features/agents/my_agents_page.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/send_message_provider.dart';
import 'package:kuzz/features/profile/presence_cache_provider.dart';
import 'package:kuzz/shared/devices/agent_host_devices.dart';
import 'package:kuzz/shared/devices/device_robot.dart';
import 'package:kuzz/shared/mentions/agent_identity_provider.dart';
import 'package:kuzz/shared/profile/user_cache_provider.dart';
import 'package:kuzz/shared/profile/user_profile.dart';
import 'package:kuzz/shared/theme/theme.dart';

final _fizz = '1' * 64;
final _honey = '2' * 64;
final _scout = '3' * 64;

Channel _channel(String id, {bool member = true, DateTime? archivedAt}) =>
    Channel(
      id: id,
      name: id,
      channelType: 'stream',
      visibility: 'open',
      description: '',
      createdBy: 'a' * 64,
      createdAt: DateTime(2026),
      memberCount: 2,
      isMember: member,
      archivedAt: archivedAt,
    );

void main() {
  group('myAgentsFrom', () {
    test('lists every hosted agent with its device', () {
      final agents = myAgentsFrom(
        hostDevices: {_honey: 'pc-b', _fizz.toUpperCase(): 'PC-A'},
        directory: const [],
        channels: const [],
      );
      expect(
        [for (final a in agents) (a.pubkey, a.deviceId)],
        [(_fizz, 'pc-a'), (_honey, 'pc-b')],
      );
      expect(agents.first.commandChannelId, isNull);
    });

    test('prefers a channel the owner is a member of', () {
      final [agent] = myAgentsFrom(
        hostDevices: {_fizz: 'pc-a'},
        directory: [
          AgentDirectoryEntry(
            pubkey: _fizz,
            displayName: ' Fizz ',
            channelIds: const ['not-mine', 'archived', 'mine'],
          ),
        ],
        channels: [
          _channel('not-mine', member: false),
          _channel('archived', archivedAt: DateTime(2026)),
          _channel('mine'),
        ],
      );
      expect(agent.commandChannelId, 'mine');
      expect(agent.directoryName, 'Fizz');
    });

    test('falls back to the agent\'s first channel', () {
      final [agent] = myAgentsFrom(
        hostDevices: {_fizz: 'pc-a'},
        directory: [
          AgentDirectoryEntry(pubkey: _fizz, channelIds: const ['c1', 'c2']),
        ],
        channels: const [],
      );
      expect(agent.commandChannelId, 'c1');
      expect(agent.directoryName, isNull);
    });
  });

  group('MyAgentsPage', () {
    Future<_RecordingSend> pump(
      WidgetTester tester, {
      required List<MyAgent> agents,
      required Map<String, String> presence,
    }) async {
      final send = _RecordingSend();
      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            myAgentsProvider.overrideWithValue(agents),
            presenceCacheProvider.overrideWith(() => _FakePresence(presence)),
            userCacheProvider.overrideWith(
              () => _FakeUsers({
                _fizz: UserProfile(pubkey: _fizz, displayName: 'Fizz'),
              }),
            ),
            agentHostDeviceNamesProvider.overrideWithValue(const {
              'pc-a': 'Office PC',
            }),
            currentAccountDeviceIdProvider.overrideWithValue('phone'),
            deviceRobotOverridesProvider.overrideWithValue(
              const <String, DeviceRobotVariant>{},
            ),
            sendMessageProvider.overrideWithValue(send),
          ],
          child: MaterialApp(
            theme: AppTheme.light(),
            home: const MyAgentsPage(),
          ),
        ),
      );
      await tester.pump();
      return send;
    }

    testWidgets('shows a hint without agents', (tester) async {
      await pump(tester, agents: const [], presence: const {});
      expect(find.byKey(const Key('my-agents-empty')), findsOneWidget);
    });

    testWidgets('names agents, their computer and state', (tester) async {
      await pump(
        tester,
        agents: [
          MyAgent(pubkey: _fizz, deviceId: 'pc-a', commandChannelId: 'c1'),
          MyAgent(pubkey: _honey, deviceId: 'pc-b', directoryName: 'Honey'),
        ],
        presence: {_fizz: 'online', _honey: 'offline'},
      );
      expect(find.text('Fizz'), findsOneWidget);
      expect(find.text('Honey'), findsOneWidget);
      expect(find.text('Running on Office PC'), findsOneWidget);
      expect(find.text('Running on another device'), findsOneWidget);
      expect(find.text('Online'), findsOneWidget);
      expect(find.text('Stopped'), findsOneWidget);
      // A stopped agent offers no Stop (and no Start: it starts on its PC).
      expect(find.byKey(ValueKey('my-agent-stop-$_honey')), findsNothing);
    });

    testWidgets('Stop sends the owner command after confirming', (
      tester,
    ) async {
      final send = await pump(
        tester,
        agents: [
          MyAgent(pubkey: _fizz, deviceId: 'pc-a', commandChannelId: 'c1'),
        ],
        presence: {_fizz: 'online'},
      );
      await tester.tap(find.byKey(ValueKey('my-agent-stop-$_fizz')));
      await tester.pumpAndSettle();
      expect(find.text('Stop Fizz?'), findsOneWidget);
      await tester.tap(find.byKey(const Key('my-agents-confirm-stop')));
      await tester.pumpAndSettle();
      expect(send.calls, [('c1', '!shutdown', _fizz)]);
      expect(find.textContaining('Stop requested'), findsOneWidget);
    });

    testWidgets('an agent in no channel cannot be stopped', (tester) async {
      await pump(
        tester,
        agents: [MyAgent(pubkey: _scout, deviceId: 'pc-a')],
        presence: {_scout: 'online'},
      );
      final button = tester.widget<TextButton>(
        find.byKey(ValueKey('my-agent-stop-$_scout')),
      );
      expect(button.onPressed, isNull);
    });
  });
}

class _FakePresence extends PresenceCacheNotifier {
  _FakePresence(this._initial);
  final Map<String, String> _initial;

  @override
  Map<String, String> build() => _initial;

  @override
  void track(List<String> pubkeys) {}
}

class _FakeUsers extends UserCacheNotifier {
  _FakeUsers(this._users);
  final Map<String, UserProfile> _users;

  @override
  Map<String, UserProfile> build() => _users;

  @override
  Future<bool> preload(List<String> pubkeys) async => true;
}

class _RecordingSend implements SendMessage {
  final calls = <(String, String, String)>[];

  @override
  Future<void> call({
    required String channelId,
    required String content,
    String? parentEventId,
    String? rootEventId,
    List<String>? mentionPubkeys,
    Channel? channel,
    List<List<String>> mediaTags = const [],
    dynamic quote,
  }) async {
    calls.add((channelId, content, (mentionPubkeys ?? const []).join(',')));
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}
