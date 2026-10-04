import 'package:buzz/shared/community/community.dart';
import 'package:buzz/shared/community/community_provider.dart';
import 'package:buzz/shared/relay/relay_provider.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

// A real 32-byte test key; never used outside tests.
const _legacyNsec =
    'nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5';
const _principal =
    'bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22';

Future<ProviderContainer> _containerFor(Community community) async {
  final container = ProviderContainer(
    overrides: [activeCommunityProvider.overrideWith((ref) async => community)],
  );
  addTearDown(container.dispose);
  await container.read(activeCommunityProvider.future);
  return container;
}

void main() {
  test(
    'a token community drops any stored nsec and uses the principal as me',
    () async {
      final container = await _containerFor(
        Community(
          id: 'c1',
          name: 'Token',
          relayUrl: 'wss://relay.example',
          pubkey: _principal,
          nsec: _legacyNsec,
          addedAt: DateTime.utc(2026),
          tokenAuth: true,
        ),
      );
      final config = container.read(relayConfigProvider);
      expect(config.tokenAuth, isTrue);
      expect(config.nsec, isNull, reason: 'token mode never signs');
      expect(config.principalId, _principal);
      expect(container.read(myPubkeyProvider), _principal);
    },
  );

  test('a legacy community keeps its nsec and derives me from it', () async {
    final container = await _containerFor(
      Community(
        id: 'c1',
        name: 'Legacy',
        relayUrl: 'wss://relay.example',
        nsec: _legacyNsec,
        addedAt: DateTime.utc(2026),
      ),
    );
    final config = container.read(relayConfigProvider);
    expect(config.tokenAuth, isFalse);
    expect(config.nsec, _legacyNsec);
    expect(container.read(myPubkeyProvider), pubkeyFromNsec(_legacyNsec));
    expect(container.read(myPubkeyProvider), isNot(_principal));
  });
}
