import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';

/// Signed local messages whose publish has not yet been corroborated by an
/// authoritative relay EVENT or query result.
class PendingLocalMessagesNotifier extends Notifier<Map<String, NostrEvent>> {
  final String channelId;

  PendingLocalMessagesNotifier(this.channelId);

  @override
  Map<String, NostrEvent> build() => const {};

  void add(NostrEvent event) {
    state = {...state, event.id: event};
  }

  NostrEvent? take(String eventId) {
    final event = state[eventId];
    if (event == null) return null;
    final next = {...state}..remove(eventId);
    state = next;
    return event;
  }

  void confirm(Iterable<String> eventIds) {
    final confirmed = eventIds.toSet();
    if (!state.keys.any(confirmed.contains)) return;
    state = {
      for (final entry in state.entries)
        if (!confirmed.contains(entry.key)) entry.key: entry.value,
    };
  }
}

/// Splits [pending] sends by signer. The map is per channel, not per account,
/// so another account's unconfirmed (e.g. failed) sends must never be shown
/// as [author]'s timeline rows; they come back when that account does.
/// Without a signed-in [author] there is nothing to hide them from.
({List<NostrEvent> own, Set<String> foreignIds}) splitLocalSendsBySigner(
  Map<String, NostrEvent> pending,
  String? author,
) {
  final signer = author?.toLowerCase();
  final own = <NostrEvent>[];
  final foreignIds = <String>{};
  for (final event in pending.values) {
    if (signer == null || event.pubkey.toLowerCase() == signer) {
      own.add(event);
    } else {
      foreignIds.add(event.id);
    }
  }
  return (own: own, foreignIds: foreignIds);
}

final pendingLocalMessagesProvider =
    NotifierProvider.family<
      PendingLocalMessagesNotifier,
      Map<String, NostrEvent>,
      String
    >(PendingLocalMessagesNotifier.new);
