import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../../shared/relay/relay.dart';
import '../../../shared/theme/theme_provider.dart';
import '../../../shared/community/community_provider.dart';
import 'channel_sections_manager.dart';
import 'channel_sections_storage.dart';

class ChannelSectionsState {
  final bool isReady;
  final ChannelSectionStore store;

  /// Bumped on every change to force downstream rebuilds.
  final int version;

  const ChannelSectionsState({
    this.isReady = false,
    this.store = const ChannelSectionStore(),
    this.version = 0,
  });
}

class ChannelSectionsNotifier extends Notifier<ChannelSectionsState> {
  ChannelSectionsManager? _manager;

  @override
  ChannelSectionsState build() {
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
      return const ChannelSectionsState();
    }

    ChannelSectionsCrypto? crypto;
    if (nsec != null) {
      try {
        crypto = ChannelSectionsCrypto(nsec, pubkey);
      } catch (_) {
        return const ChannelSectionsState();
      }
    }

    final prefs = ref.read(savedPrefsProvider);
    final signedRelay = nsec == null
        ? null
        : SignedEventRelay(
            session: ref.read(relaySessionProvider.notifier),
            nsec: nsec,
          );

    late final ChannelSectionsManager manager;
    manager = ChannelSectionsManager(
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

    return ChannelSectionsState(
      isReady: false,
      store: manager.store,
      version: 1,
    );
  }

  void createSection(String name) => _mutate((m) => m.createSection(name));

  void renameSection(String sectionId, String newName) =>
      _mutate((m) => m.renameSection(sectionId, newName));

  void deleteSection(String sectionId) =>
      _mutate((m) => m.deleteSection(sectionId));

  void moveSectionUp(String sectionId) =>
      _mutate((m) => m.moveSectionUp(sectionId));

  void moveSectionDown(String sectionId) =>
      _mutate((m) => m.moveSectionDown(sectionId));

  void assignChannel(String channelId, String sectionId) =>
      _mutate((m) => m.assignChannel(channelId, sectionId));

  void unassignChannel(String channelId) =>
      _mutate((m) => m.unassignChannel(channelId));

  /// Apply a local edit and expose it at once: the manager only reports
  /// changes from relay sync, which never runs while offline or on a
  /// local-only (token) community.
  void _mutate(void Function(ChannelSectionsManager manager) edit) {
    final manager = _manager;
    if (manager == null) return;
    edit(manager);
    if (_manager != manager) return;
    state = ChannelSectionsState(
      isReady: state.isReady,
      store: manager.store,
      version: state.version + 1,
    );
  }

  void _emitManagerState(ChannelSectionsManager manager) {
    if (_manager != manager) return;
    state = ChannelSectionsState(
      isReady: true,
      store: manager.store,
      version: state.version + 1,
    );
  }
}

final channelSectionsProvider =
    NotifierProvider<ChannelSectionsNotifier, ChannelSectionsState>(
      ChannelSectionsNotifier.new,
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
