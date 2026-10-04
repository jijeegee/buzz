import 'package:buzz/shared/community/community.dart';
import 'package:buzz/shared/community/community_provider.dart';
import 'package:buzz/shared/push/push_relay_capability_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

const _principal =
    'bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22';

/// The push lease (kind 30350), its revocation, and the iOS NSE's NIP-98 /
/// NIP-44 handling all need the community's signing key. A token community
/// has none, so push must be reported as unavailable — never discovered,
/// enrolled, or leased — instead of failing later on a missing key.
void main() {
  test('a token community never discovers relay push capability', () async {
    var fetches = 0;
    final container = ProviderContainer(
      overrides: [
        relayConfigProvider.overrideWith(_TokenConfigNotifier.new),
        relaySessionProvider.overrideWith(_ConnectedSession.new),
        myPubkeyProvider.overrideWithValue(_principal),
        activeCommunityProvider.overrideWith(
          (ref) async => Community(
            id: 'community-1',
            name: 'Token relay',
            relayUrl: 'https://relay.example.com',
            pubkey: _principal,
            tokenAuth: true,
            addedAt: DateTime.utc(2026),
          ),
        ),
        buzzPushDescriptorFetcherProvider.overrideWithValue((_) async {
          fetches++;
          throw StateError('must not be fetched');
        }),
      ],
    );
    addTearDown(container.dispose);
    await container.read(activeCommunityProvider.future);

    final descriptor = await container.read(
      currentRelayPushDescriptorProvider.future,
    );

    expect(descriptor, isNull);
    expect(fetches, 0);
  });
}

class _TokenConfigNotifier extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(
    baseUrl: 'https://relay.example.com',
    tokenAuth: true,
    principalId: _principal,
  );
}

class _ConnectedSession extends RelaySessionNotifier {
  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);
}
