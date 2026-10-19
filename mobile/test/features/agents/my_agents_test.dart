import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/agents/my_agents.dart';
import 'package:kuzz/shared/mentions/agent_identity_provider.dart';

final _fizz = '1' * 64;
final _honey = '2' * 64;

void main() {
  group('myAgentsFrom', () {
    test('lists every hosted agent with its device', () {
      final agents = myAgentsFrom(
        hostDevices: {_honey: 'pc-b', _fizz.toUpperCase(): 'PC-A'},
        directory: const [],
      );
      expect(
        [for (final a in agents) (a.pubkey, a.deviceId)],
        [(_fizz, 'pc-a'), (_honey, 'pc-b')],
      );
      expect(agents.first.directoryName, isNull);
    });

    test('names an agent from its directory profile', () {
      final [agent] = myAgentsFrom(
        hostDevices: {_fizz: 'pc-a'},
        directory: [AgentDirectoryEntry(pubkey: _fizz, displayName: ' Fizz ')],
      );
      expect(agent.directoryName, 'Fizz');
    });
  });
}
