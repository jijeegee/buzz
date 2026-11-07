import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/shared/auth/account/account_api.dart';
import 'package:kuzz/shared/auth/token/session_key.dart';
import 'package:kuzz/shared/mentions/agent_identity_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';

final _session = SessionKey('https://relay.test', 'me');
final _me = 'ab' * 32;
final _agent = 'cd' * 32;

ProviderContainer _container({
  bool hasAccount = true,
  required Future<List<AccountDevice>> Function() devices,
}) {
  final container = ProviderContainer(
    overrides: [
      agentDirectoryProvider.overrideWith((ref) async => const []),
      accountSessionProvider.overrideWithValue(hasAccount ? _session : null),
      myPubkeyProvider.overrideWithValue(_me),
      accountDevicesProvider(_session).overrideWith((ref) => devices()),
    ],
  );
  addTearDown(container.dispose);
  return container;
}

void main() {
  final device = AccountDevice.fromJson({
    'id': 'device-1',
    'name': 'host',
    'platform': 'external',
    'current': false,
    'agents': [
      {'pubkey': _agent.toUpperCase(), 'runtime': 'Hermes'},
    ],
  });

  test('a paired agent is mine, with an empty directory too', () async {
    final container = _container(devices: () async => [device]);
    expect(await container.read(agentOwnersProvider.future), {_agent: _me});
    expect(container.read(knownAgentPubkeysProvider), contains(_agent));
  });

  test('a device list that fails adds none and is no error', () async {
    final container = _container(devices: () => Future.error('offline'));
    expect(await container.read(agentOwnersProvider.future), isEmpty);
  });

  test('a slow device list still counts once it arrives', () async {
    final slow = Completer<List<AccountDevice>>();
    final container = _container(devices: () => slow.future);
    final sub = container.listen(agentOwnersProvider, (_, _) {});
    addTearDown(sub.close);
    expect(await container.read(agentOwnersProvider.future), isEmpty);

    slow.complete([device]);
    await Future<void>.delayed(const Duration(milliseconds: 50));

    expect(await container.read(agentOwnersProvider.future), {_agent: _me});
  });

  test('a device list that hangs does not hold the owners up', () async {
    final container = _container(
      devices: () => Future.delayed(const Duration(hours: 1), () => [device]),
    );
    final watch = Stopwatch()..start();
    expect(await container.read(agentOwnersProvider.future), isEmpty);
    expect(watch.elapsed, lessThan(const Duration(seconds: 10)));
  });

  test('a community without an account has none', () async {
    final container = _container(
      hasAccount: false,
      devices: () async => [device],
    );
    expect(await container.read(agentOwnersProvider.future), isEmpty);
  });
}
