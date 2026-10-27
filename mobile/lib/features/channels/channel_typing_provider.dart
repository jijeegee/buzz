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
/// Subscribes to kind:20002 (typing indicator) events via websocket while a
/// screen of the room watches it; leaving the room closes the subscription.
/// Entries expire after 8 seconds (matching the desktop TTL).
class ChannelTypingNotifier extends Notifier<List<TypingEntry>> {
  static const _ttlMs = 8000;
  static const _pruneIntervalMs = 1000;

  final String channelId;
  Timer? _pruneTimer;

  ChannelTypingNotifier(this.channelId);

  @override
  List<TypingEntry> build() {
    final connected = ref.watch(
      relaySessionProvider.select(
        (session) => session.status == SessionStatus.connected,
      ),
    );
    var disposed = false;
    void Function()? unsubscribe;

    ref.onDispose(() {
      disposed = true;
      unsubscribe?.call();
      _pruneTimer?.cancel();
      _pruneTimer = null;
    });

    if (connected) {
      unawaited(() async {
        final stop = await ref
            .read(relaySessionProvider.notifier)
            .subscribe(
              NostrFilter(
                kinds: [EventKind.typingIndicator],
                tags: {
                  '#h': [channelId],
                },
                limit: 10,
              ),
              _handleTypingEvent,
            );
        // Left the room (or reconnected) while the REQ was opening.
        if (disposed) {
          stop();
        } else {
          unsubscribe = stop;
        }
      }());
    }

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
