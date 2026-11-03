import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';

/// A single typing indicator entry.
@immutable
class TypingEntry {
  final String pubkey;
  final String? threadHeadId;
  final int expiresAtMs;

  const TypingEntry({
    required this.pubkey,
    this.threadHeadId,
    required this.expiresAtMs,
  });
}

/// Tracks who is currently typing in a specific channel.
///
/// Listens to kind:20002 (typing indicator) events on the room's open-room
/// REQ ([RoomLiveRegistry]) while a screen of the room watches it; leaving
/// the room closes it. A reconnect does not rebuild this: the session's
/// replay re-sends the REQ. Entries expire after 8 seconds (matching the
/// desktop TTL).
class ChannelTypingNotifier extends Notifier<List<TypingEntry>> {
  static const _ttlMs = 8000;
  static const _pruneIntervalMs = 1000;

  final String channelId;
  Timer? _pruneTimer;

  ChannelTypingNotifier(this.channelId);

  @override
  List<TypingEntry> build() {
    final stop = ref
        .watch(roomLiveRegistryProvider)
        .listen(channelId, EventKind.typingIndicator, _handleTypingEvent);
    ref.onDispose(() {
      stop();
      _pruneTimer?.cancel();
      _pruneTimer = null;
    });
    return [];
  }

  void _handleTypingEvent(NostrEvent event) {
    final now = DateTime.now().millisecondsSinceEpoch;
    final entry = TypingEntry(
      pubkey: event.pubkey,
      threadHeadId: event.getTagValue('e'),
      expiresAtMs: now + _ttlMs,
    );

    // Upsert: replace existing entry for same pubkey+thread, or add.
    final updated =
        state
            .where(
              (e) =>
                  !(e.pubkey == entry.pubkey &&
                      e.threadHeadId == entry.threadHeadId),
            )
            .toList()
          ..add(entry);

    state = updated;
    _ensurePruneTimer();
  }

  void _ensurePruneTimer() {
    _pruneTimer ??= Timer.periodic(
      const Duration(milliseconds: _pruneIntervalMs),
      (_) => _prune(),
    );
  }

  void _prune() {
    final now = DateTime.now().millisecondsSinceEpoch;
    final pruned = state.where((e) => e.expiresAtMs > now).toList();
    state = pruned;

    if (pruned.isEmpty) {
      _pruneTimer?.cancel();
      _pruneTimer = null;
    }
  }
}

final channelTypingProvider = NotifierProvider.autoDispose
    .family<ChannelTypingNotifier, List<TypingEntry>, String>(
      ChannelTypingNotifier.new,
    );
