import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import 'room_live_router.dart';
import 'thread_name_provider.dart';

/// Names asked for within this window share one read.
const threadNameBatchWindow = Duration(milliseconds: 50);

/// The relay accepts at most 128 `#h` values per query.
const threadNameBatchRooms = 128;

/// Thread heads per read.
const threadNameBatchIds = 100;

/// Events per page.
const threadNamePageLimit = 200;

/// Pages per read before the rest is left unresolved.
const threadNameMaxPages = 10;

typedef ThreadNameQuery = Future<List<NostrEvent>> Function(NostrFilter filter);

class _Request {
  _Request(this.key);

  final ThreadNameKey key;
  final waiters = <Completer<NostrEvent?>>[];
}

/// Thread names for many rows in a few HTTP reads, instead of one read and
/// one live REQ per row. Mirrors desktop's `ThreadNameBatcher`:
///
/// - Names asked for within [threadNameBatchWindow] are read together, once
///   their rooms are covered (`whenRoomCovered`), so a rename between the
///   read and the live stream cannot be lost.
/// - One read is one filter `{kinds:[40009], #h:[≤128 rooms], #e:[≤100
///   heads], limit:200}`; [latestThreadName] picks per (room, head).
/// - Pages go back with an inclusive `until` and drop heads already named
///   from `#e`. A short page ends the read (the rest have no name); after
///   [threadNameMaxPages] the rest resolve to null and callers keep what they
///   hold. A failed read fails every caller in it.
class ThreadNameBatcher {
  ThreadNameBatcher({
    required ThreadNameQuery query,
    required Future<void> Function(String roomId) whenRoomCovered,
    Duration window = threadNameBatchWindow,
  }) : _query = query,
       _whenRoomCovered = whenRoomCovered,
       _window = window;

  final ThreadNameQuery _query;
  final Future<void> Function(String roomId) _whenRoomCovered;
  final Duration _window;
  final _queued = <ThreadNameKey, _Request>{};
  Timer? _timer;

  Future<NostrEvent?> load(ThreadNameKey key) {
    final request = _queued.putIfAbsent(key, () => _Request(key));
    final completer = Completer<NostrEvent?>();
    request.waiters.add(completer);
    _timer ??= Timer(_window, _flush);
    return completer.future;
  }

  void dispose() {
    _timer?.cancel();
    _timer = null;
    for (final request in _queued.values) {
      for (final waiter in request.waiters) {
        waiter.completeError(StateError('Thread names were reset.'));
      }
    }
    _queued.clear();
  }

  void _flush() {
    _timer = null;
    final requests = _queued.values.toList();
    _queued.clear();
    for (final bucket in _buckets(requests)) {
      unawaited(_read(bucket));
    }
  }

  Future<void> _read(List<_Request> bucket) async {
    try {
      await Future.wait(
        {
          for (final request in bucket) request.key.channelId,
        }.map(_whenRoomCovered),
      );
      final names = await _readPages(bucket);
      for (final request in bucket) {
        for (final waiter in request.waiters) {
          waiter.complete(names[request.key]);
        }
      }
    } catch (error, stack) {
      for (final request in bucket) {
        for (final waiter in request.waiters) {
          waiter.completeError(error, stack);
        }
      }
    }
  }

  Future<Map<ThreadNameKey, NostrEvent>> _readPages(
    List<_Request> bucket,
  ) async {
    final names = <ThreadNameKey, NostrEvent>{};
    final seen = <String>{};
    var open = bucket;
    int? until;
    for (var page = 0; page < threadNameMaxPages; page++) {
      final events = await _query(
        NostrFilter(
          kinds: const [EventKind.threadName],
          tags: {
            '#h': {for (final request in open) request.key.channelId}.toList(),
            '#e': [for (final request in open) request.key.headId],
          },
          limit: threadNamePageLimit,
          until: until,
        ),
      );
      int? oldest;
      final fresh = <NostrEvent>[];
      for (final event in events) {
        oldest = oldest == null
            ? event.createdAt
            : (event.createdAt < oldest ? event.createdAt : oldest);
        if (seen.add(event.id)) fresh.add(event);
      }
      for (final request in open) {
        final next = latestThreadName(request.key, [
          ?names[request.key],
          ...fresh,
        ]);
        if (next != null) names[request.key] = next;
      }
      open = [
        for (final request in open)
          if (!names.containsKey(request.key)) request,
      ];
      if (events.length < threadNamePageLimit ||
          open.isEmpty ||
          oldest == null) {
        return names;
      }
      // `until` is inclusive: a page of one second only repeats itself.
      until = fresh.isEmpty || oldest == until ? oldest - 1 : oldest;
    }
    debugPrint(
      '[thread-name] page cap reached; names left unresolved: ${open.length}',
    );
    return names;
  }
}

/// Requests grouped into reads of ≤128 rooms and ≤100 heads.
List<List<_Request>> _buckets(List<_Request> requests) {
  final sorted = [...requests]
    ..sort((a, b) {
      final room = a.key.channelId.compareTo(b.key.channelId);
      return room != 0 ? room : a.key.headId.compareTo(b.key.headId);
    });
  final result = <List<_Request>>[];
  var current = <_Request>[];
  var rooms = <String>{};
  for (final request in sorted) {
    final addsRoom = !rooms.contains(request.key.channelId);
    if (current.length >= threadNameBatchIds ||
        (addsRoom && rooms.length >= threadNameBatchRooms)) {
      result.add(current);
      current = [];
      rooms = {};
    }
    current.add(request);
    rooms.add(request.key.channelId);
  }
  if (current.isNotEmpty) result.add(current);
  return result;
}

/// The active relay session's thread-name batcher. Rebuilt with the relay or
/// account.
final threadNameBatcherProvider = Provider<ThreadNameBatcher>((ref) {
  ref.watch(relayConfigProvider.select((config) => config.baseUrl));
  ref.watch(myPubkeyProvider);
  final router = ref.watch(roomLiveRouterProvider);
  final batcher = ThreadNameBatcher(
    query: (filter) =>
        ref.read(relaySessionProvider.notifier).queryRelay([filter]),
    whenRoomCovered: router.whenRoomCovered,
  );
  ref.onDispose(batcher.dispose);
  return batcher;
});
