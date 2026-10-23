import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import 'local_send_store.dart';

/// Signed local messages whose publish has not yet been corroborated by an
/// authoritative relay EVENT or query result.
///
/// One added with `persist` (a composer send) is also kept in
/// [LocalSendStore] until it is confirmed, accepted or dropped, so an app
/// restart can show it again.
class PendingLocalMessagesNotifier extends Notifier<Map<String, NostrEvent>> {
  final String channelId;
  final _storeKeys = <String, String>{};
  final _readStoreKeys = <String>{};
  final _unverified = <String>{};

  PendingLocalMessagesNotifier(this.channelId);

  @override
  Map<String, NostrEvent> build() => const {};

  void add(NostrEvent event, {bool persist = false}) {
    state = {...state, event.id: event};
    final store = persist ? ref.read(localSendStoreProvider) : null;
    if (store == null) return;
    // Removal uses the key it was stored under, even after a switch.
    final key = _storeKeys[event.id] = store.keyFor(event.pubkey, channelId);
    store.put(key, event);
  }

  NostrEvent? take(String eventId) {
    final event = state[eventId];
    if (event == null) return null;
    final next = {...state}..remove(eventId);
    state = next;
    _forget({eventId});
    return event;
  }

  void confirm(Iterable<String> eventIds) {
    final confirmed = eventIds.toSet();
    if (!state.keys.any(confirmed.contains)) return;
    state = {
      for (final entry in state.entries)
        if (!confirmed.contains(entry.key)) entry.key: entry.value,
    };
    _forget(confirmed);
  }

  /// [signer]'s stored sends for this channel that are not pending here yet,
  /// already tied to their store entry: add them without `persist`.
  /// Each store entry is read once; after that this map is its live copy.
  List<NostrEvent> storedSends(String signer) {
    final store = ref.read(localSendStoreProvider);
    final key = store?.keyFor(signer, channelId);
    if (store == null || !_readStoreKeys.add(key!)) return const [];
    final sends = [
      for (final event in store.read(key))
        if (!state.containsKey(event.id) &&
            event.pubkey.toLowerCase() == signer.toLowerCase())
          event,
    ];
    for (final event in sends) {
      _storeKeys[event.id] = key;
    }
    return sends;
  }

  /// Restored sends the relay could not be asked about: they may have landed.
  void markUnverified(Iterable<String> eventIds) =>
      _unverified.addAll(eventIds.where(state.containsKey));

  bool isUnverified(String eventId) => _unverified.contains(eventId);

  void markVerified(String eventId) => _unverified.remove(eventId);

  void _forget(Set<String> eventIds) {
    _unverified.removeAll(eventIds);
    final store = ref.read(localSendStoreProvider);
    for (final key in {for (final id in eventIds) ?_storeKeys.remove(id)}) {
      store?.remove(key, eventIds);
    }
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
