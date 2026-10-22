import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../../shared/community/community_provider.dart';
import '../../../shared/relay/relay.dart';
import '../../../shared/theme/theme_provider.dart';
import '../channel_mutes/channel_mutes_manager.dart' show ChannelMutesCrypto;
import 'channel_names_manager.dart';
import 'channel_names_storage.dart';

/// My private chat names (Rename for me), synced across my devices.
class ChannelNamesNotifier extends Notifier<ChannelNameStore> {
  ChannelNamesManager? _manager;

  @override
  ChannelNameStore build() {
    _manager?.dispose();
    _manager = null;

    final relayConfig = ref.watch(relayConfigProvider);
    final sessionState = ref.watch(relaySessionProvider);
    ref.watch(activeCommunityProvider);

    // A token community has no key to encrypt with: names stay on this
    // device under the principal id.
    final trimmed = relayConfig.tokenAuth ? null : relayConfig.nsec?.trim();
    final nsec = trimmed == null || trimmed.isEmpty ? null : trimmed;
    final pubkey = nsec == null
        ? (relayConfig.tokenAuth ? relayConfig.principalId : null)
        : _safePubkeyFromNsec(nsec);
    if (pubkey == null || pubkey.isEmpty) return const ChannelNameStore();

    ChannelMutesCrypto? crypto;
    if (nsec != null) {
      try {
        crypto = ChannelMutesCrypto(nsec, pubkey);
      } catch (_) {
        crypto = null;
      }
    }

    late final ChannelNamesManager manager;
    manager = ChannelNamesManager(
      pubkey: pubkey,
      prefs: ref.read(savedPrefsProvider),
      crypto: crypto,
      relaySession: ref.read(relaySessionProvider.notifier),
      signedEventRelay: nsec == null
          ? null
          : SignedEventRelay(
              session: ref.read(relaySessionProvider.notifier),
              nsec: nsec,
            ),
      remoteEnabled: sessionState.status == SessionStatus.connected,
      onChanged: () {
        if (_manager == manager) state = manager.store;
      },
    );
    _manager = manager;

    ref.listen(appLifecycleProvider, (_, next) {
      if (next == AppLifecycleState.resumed) {
        manager.refreshFromRelay().ignore();
      }
    });
    ref.onDispose(() {
      manager.dispose();
      if (_manager == manager) _manager = null;
    });
    Future.microtask(manager.initialize);
    return manager.store;
  }

  /// Sets my private name for [channelId]; null or blank restores it.
  void setName(String channelId, String? name) =>
      _manager?.setName(channelId, name);
}

final channelNamesProvider =
    NotifierProvider<ChannelNamesNotifier, ChannelNameStore>(
      ChannelNamesNotifier.new,
    );

/// My private name for one chat, or null.
final channelPrivateNameProvider = Provider.family<String?, String>(
  (ref, channelId) =>
      ref.watch(channelNamesProvider.select((s) => s.nameFor(channelId))),
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
