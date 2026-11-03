import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../message_store/message_store.dart' show MessageStoreLease;
import '../relay/nostr_models.dart';

/// What the screens learn of push inbox drains
/// (docs/message-sync.md#push-inbox), besides the list folds.

/// Bumped after a drain wrote at least one file into the store, so store
/// paints still drawn before the relay's answer read the store again.
/// Watched rather than invalidated: a dependency change does not dispose a
/// provider nobody listens to.
class PushInboxRevisionNotifier extends Notifier<int> {
  @override
  int build() => 0;

  void bump() => state++;
}

final pushInboxRevisionProvider =
    NotifierProvider<PushInboxRevisionNotifier, int>(
      PushInboxRevisionNotifier.new,
    );

/// The pushed events of listed rooms the store accepted under the current
/// lease, oldest first, for the Chats list's saved rows: those can be older
/// than the drain, and drawn after it. Each drain is numbered, so a first
/// page drops the events drained before it was asked for (the relay's page
/// has them). Only in memory, never saved.
class PushedChatEvents {
  /// The most events kept; the oldest go first.
  static const cap = 500;

  MessageStoreLease? _lease;
  int _drains = 0;
  final _events = <({int drain, NostrEvent event})>[];

  /// The number of the last drain added.
  int get drains => _drains;

  /// Adds one drain's [events]; another lease's are dropped first.
  void add(MessageStoreLease lease, List<NostrEvent> events) {
    if (_lease != lease) {
      _lease = lease;
      _events.clear();
    }
    final drain = ++_drains;
    final known = {for (final entry in _events) entry.event.id};
    for (final event in events) {
      if (known.add(event.id)) _events.add((drain: drain, event: event));
    }
    _events.sort((a, b) => a.event.createdAt.compareTo(b.event.createdAt));
    if (_events.length > cap) _events.removeRange(0, _events.length - cap);
  }

  /// [lease]'s events drained after drain [after].
  List<NostrEvent> after(MessageStoreLease? lease, [int after = 0]) => [
    if (lease != null && lease == _lease)
      for (final entry in _events)
        if (entry.drain > after) entry.event,
  ];

  /// Drops [lease]'s events drained up to drain [through].
  void dropThrough(MessageStoreLease? lease, int through) {
    if (lease == null || lease != _lease) return;
    _events.removeWhere((entry) => entry.drain <= through);
  }
}

final pushedChatEventsProvider = Provider<PushedChatEvents>(
  (ref) => PushedChatEvents(),
);
