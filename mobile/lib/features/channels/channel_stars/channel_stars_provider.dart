import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../../shared/relay/relay.dart';
import '../../../shared/theme/theme_provider.dart';
import '../../../shared/community/community_provider.dart';
import 'channel_stars_manager.dart';
import 'channel_stars_storage.dart';

class ChannelStarsState {
  final bool isReady;
  final ChannelStarStore store;

  /// Bumped on every change to force downstream rebuilds.
  final int version;

  const ChannelStarsState({
    this.isReady = false,
    this.store = const ChannelStarStore(),
    this.version = 0,
  });
}

class ChannelStarsNotifier extends Notifier<ChannelStarsState> {
  ChannelStarsManager? _manager;

  @override
  ChannelStarsState build() {
    _manager?.dispose(flushPending: false);
    _manager = null;

    final relayConfig = ref.watch(relayConfigProvider);
    final sessionState = ref.watch(relaySessionProvider);
    // Rebuild when the active community changes (pubkey may differ).
    ref.watch(activeCommunityProvider);

    // A token community has no nsec, so no NIP-44 key: the setting is kept
    // local-only on this device under the principal pubkey.
    final trimmed = relayConfig.tokenAuth ? null : relayConfig.nsec?.trim();
    final nsec = trimmed == null || trimmed.isEmpty ? null : trimmed;
    final pubkey = nsec == null
        ? (relayConfig.tokenAuth ? relayConfig.principalId : null)
        : _safePubkeyFromNsec(nsec);
    if (pubkey == null || pubkey.isEmpty) {
      return const ChannelStarsState();
    }

    ChannelStarsCrypto? crypto;
    if (nsec != null) {
      try {
        crypto = ChannelStarsCrypto(nsec, pubkey);
      } catch (_) {
        return const ChannelStarsState();
      }
    }

    final prefs = ref.read(savedPrefsProvider);
    final signedRelay = nsec == null
        ? null
        : SignedEventRelay(
            session: ref.read(relaySessionProvider.notifier),
            nsec: nsec,
          );

    late final ChannelStarsManager manager;
    manager = ChannelStarsManager(
      pubkey: pubkey,
      prefs: prefs,
      crypto: crypto,
      relaySession: ref.read(relaySessionProvider.notifier),
      signedEventRelay: signedRelay,
      remoteEnabled: sessionState.status == SessionStatus.connected,
      onChanged: () => _emitManagerState(manager),
    );
    _manager = manager;

    // Resume re-read catches an EVENT a healthy socket never delivered.
    ref.listen(appLifecycleProvider, (_, next) {
      if (next == AppLifecycleState.resumed) manager.refreshFromRelay();
    });

    ref.onDispose(() {
      manager.dispose();
      if (_manager == manager) {
        _manager = null;
      }
    });

    Future.microtask(() async {
      await manager.initialize();
      if (_manager != manager) return;
      _emitManagerState(manager);
    });

    return ChannelStarsState(isReady: false, store: manager.store, version: 1);
  }

  void starChannel(String channelId) => _manager?.starChannel(channelId);

  void unstarChannel(String channelId) => _manager?.unstarChannel(channelId);

  void _emitManagerState(ChannelStarsManager manager) {
    if (_manager != manager) return;
    state = ChannelStarsState(
      isReady: true,
      store: manager.store,
      version: state.version + 1,
    );
  }
}

final channelStarsProvider =
    NotifierProvider<ChannelStarsNotifier, ChannelStarsState>(
      ChannelStarsNotifier.new,
    );

String? _safePubkeyFromNsec(String nsec) {
  try {
    final privkeyHex = nostr.Nip19.decode(payload: nsec).data;
    if (privkeyHex.isEmpty) return null;
    return nostr.Keys(privkeyHex).public;
  } catch (_) {
    return null;
  }
}
