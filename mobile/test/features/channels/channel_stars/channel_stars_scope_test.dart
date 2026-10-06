import 'dart:async';

import 'package:buzz/features/channels/channel_stars/channel_stars_manager.dart';
import 'package:buzz/features/channels/channel_stars/channel_stars_storage.dart';
import 'package:buzz/features/channels/channel_stars/channel_stars_provider.dart';
import 'package:buzz/shared/community/community_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/theme/theme_provider.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../sidebar_sync_fixture.dart';

void main() {
  test(
    'failed legacy migration preserves data and exposes a retryable provider error',
    () async {
      final backing = await freshPrefs();
      final prefs = _MigrationPrefs(backing);
      await ChannelStarsStorage(backing).write(
        'account',
        const ChannelStarStore(
          channels: {'kept': ChannelStarEntry(starred: true, updatedAt: 1)},
        ),
      );
      final container = ProviderContainer(
        overrides: [
          savedPrefsProvider.overrideWithValue(prefs),
          relayConfigProvider.overrideWith(_ScopeConfig.new),
          relaySessionProvider.overrideWith(_OfflineSession.new),
          activeCommunityProvider.overrideWith((ref) async => null),
        ],
      );
      addTearDown(container.dispose);
      await container.read(activeCommunityProvider.future);
      prefs.failures = 1;
      container.listen(channelStarsProvider, (_, _) {});
      await pumpEventQueue();
      expect(container.read(channelStarsProvider).errorMessage, isNotNull);
      expect(container.read(channelStarsProvider).isReady, isFalse);
      expect(
        ChannelStarsStorage(backing).read('account').channels['kept']?.starred,
        isTrue,
      );
      container.invalidate(channelStarsProvider);
      await pumpEventQueue();
      expect(container.read(channelStarsProvider).errorMessage, isNull);
      expect(
        container.read(channelStarsProvider).store.channels['kept']?.starred,
        isTrue,
      );
    },
  );
  test(
    'legacy pins migrate once, survive restart, and isolate origin/account',
    () async {
      final prefs = await freshPrefs();
      final storage = ChannelStarsStorage(prefs);
      const pins = ChannelStarStore(
        channels: {'shared-id': ChannelStarEntry(starred: true, updatedAt: 1)},
      );
      await storage.write('account-a', pins);
      expect(
        (await storage.load(
          'account-a',
          origin: 'wss://ONE.example:443/',
        )).channels['shared-id']?.starred,
        isTrue,
      );
      final restarted = ChannelStarsStorage(prefs);
      expect(
        (await restarted.load(
          'account-a',
          origin: 'https://one.example',
        )).channels['shared-id']?.starred,
        isTrue,
      );
      expect(
        (await restarted.load(
          'account-a',
          origin: 'https://two.example',
        )).channels,
        isEmpty,
      );
      expect(
        (await restarted.load(
          'account-b',
          origin: 'https://one.example',
        )).channels,
        isEmpty,
      );
      expect(storage.read('account-a').channels['shared-id']?.starred, isTrue);
    },
  );

  fakeAsyncTest(
    'late pin history from disposed community cannot repopulate the next scope',
    (clock) {
      final relay = SidebarRelay();
      late ChannelStarsManager previous;
      late ChannelStarsManager current;
      var changed = 0;
      freshPrefs().then((prefs) {
        relay.stored.add(
          relay.event(
            'channel-stars',
            const ChannelStarStore(
              channels: {
                'same-id': ChannelStarEntry(starred: true, updatedAt: 10),
              },
            ).toJson(),
            10,
          ),
        );
        relay.holdHistory = Completer<void>();
        previous = ChannelStarsManager(
          pubkey: relay.pubkey,
          origin: 'https://previous.example',
          prefs: prefs,
          crypto: ChannelStarsCrypto(relay.keys.nsec, relay.pubkey),
          relaySession: relay.session,
          signedEventRelay: relay.signer,
          remoteEnabled: true,
          onChanged: () => changed++,
        );
        unawaited(previous.initialize());
        clock.flushMicrotasks();
        previous.dispose(flushPending: false);
        current = ChannelStarsManager(
          pubkey: relay.pubkey,
          origin: 'https://current.example',
          prefs: prefs,
          crypto: null,
          relaySession: null,
          signedEventRelay: null,
          remoteEnabled: false,
          onChanged: () {},
        );
        unawaited(current.initialize());
        relay.holdHistory!.complete();
      });
      clock.flushMicrotasks();
      expect(current.store.channels, isEmpty);
      expect(changed, 0);
      expect(relay.published, isEmpty);
      current.dispose(flushPending: false);
      relay.session.debugDispose();
    },
  );

  fakeAsyncTest(
    'disposed pending pin publish cannot send on switched relay session',
    (clock) {
      final relay = SidebarRelay();
      late ChannelStarsManager manager;
      freshPrefs().then((prefs) {
        manager = ChannelStarsManager(
          pubkey: relay.pubkey,
          origin: 'https://previous.example',
          prefs: prefs,
          crypto: ChannelStarsCrypto(relay.keys.nsec, relay.pubkey),
          relaySession: relay.session,
          signedEventRelay: relay.signer,
          remoteEnabled: true,
          onChanged: () {},
        );
        unawaited(manager.initialize());
      });
      clock.flushMicrotasks();
      manager.starChannel('same-id');
      relay.holdHistory = Completer<void>();
      clock.elapse(const Duration(seconds: 5));
      manager.dispose(flushPending: false);
      relay.holdHistory!.complete();
      clock.flushMicrotasks();
      expect(relay.published, isEmpty);
      relay.session.debugDispose();
    },
  );
}

class _MigrationPrefs extends FlakyPrefs {
  _MigrationPrefs(super.inner);
  @override
  bool containsKey(String key) => getString(key) != null;
}

class _ScopeConfig extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(
    baseUrl: 'https://one.example',
    tokenAuth: true,
    principalId: 'account',
  );
}

class _OfflineSession extends RelaySessionNotifier {
  @override
  SessionState build() =>
      const SessionState(status: SessionStatus.disconnected);
}
