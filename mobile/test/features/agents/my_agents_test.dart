import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/agents/my_agents.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/shared/mentions/agent_identity_provider.dart';

final _fizz = '1' * 64;
final _honey = '2' * 64;

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
}
