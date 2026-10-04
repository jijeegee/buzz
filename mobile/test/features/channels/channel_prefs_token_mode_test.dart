import 'package:buzz/features/channels/channel_mutes/channel_mutes_provider.dart';
import 'package:buzz/features/channels/channel_sections/channel_sections_provider.dart';
import 'package:buzz/features/channels/channel_sort/channel_sort_provider.dart';
import 'package:buzz/features/channels/channel_sort/channel_sort_storage.dart';
import 'package:buzz/features/channels/channel_stars/channel_stars_provider.dart';
import 'package:buzz/shared/community/community.dart';
import 'package:buzz/shared/community/community_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/theme/theme_provider.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

const _principal =
    'bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22';

/// A token community has no nsec, so the NIP-44 encrypted settings cannot
/// sync. They must still work on this device (local-only), not silently
/// drop every edit.
void main() {
  late ProviderContainer container;
  late _RecordingRelaySession session;

  setUp(() async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    session = _RecordingRelaySession();
    container = ProviderContainer(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        relayConfigProvider.overrideWith(_TokenConfigNotifier.new),
        relaySessionProvider.overrideWith(() => session),
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
      ],
    );
    addTearDown(container.dispose);
    await container.read(activeCommunityProvider.future);
  });

  test('mutes apply locally', () async {
    container.listen(channelMutesProvider, (_, _) {});
    await pumpEventQueue();
    container.read(channelMutesProvider.notifier).muteChannel('c1');
    await pumpEventQueue();

    final state = container.read(channelMutesProvider);
    expect(state.isReady, isTrue);
    expect(state.store.channels['c1']?.muted, isTrue);
    expect(session.published, isEmpty);
  });

  test('stars apply locally', () async {
    container.listen(channelStarsProvider, (_, _) {});
    await pumpEventQueue();
    container.read(channelStarsProvider.notifier).starChannel('c1');
    await pumpEventQueue();

    final state = container.read(channelStarsProvider);
    expect(state.isReady, isTrue);
    expect(state.store.channels['c1']?.starred, isTrue);
    expect(session.published, isEmpty);
  });

  test('sections apply locally', () async {
    container.listen(channelSectionsProvider, (_, _) {});
    await pumpEventQueue();
    container.read(channelSectionsProvider.notifier).createSection('Work');
    await pumpEventQueue();

    final state = container.read(channelSectionsProvider);
    expect(state.isReady, isTrue);
    expect(state.store.sections.map((s) => s.name), ['Work']);
    expect(session.published, isEmpty);
  });

  test('sort modes apply locally', () async {
    container.listen(channelSortProvider, (_, _) {});
    await pumpEventQueue();
    container
        .read(channelSortProvider.notifier)
        .setSortModeFor('channels', ChannelSortMode.recent);
    await pumpEventQueue();

    final state = container.read(channelSortProvider);
    expect(state.isReady, isTrue);
    expect(state.store.groups['channels'], ChannelSortMode.recent);
    expect(session.published, isEmpty);
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

class _RecordingRelaySession extends RelaySessionNotifier {
  final List<NostrEvent> published = [];

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<NostrEvent> publish(
    NostrEvent event, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    published.add(event);
    return event;
  }

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async => const [];

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async => const [];
}
