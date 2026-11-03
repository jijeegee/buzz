import 'dart:async';

import 'nostr_models.dart';

/// Kind of the account's own settings lanes (mutes, stars, theme, ...).
const _laneKind = 30078;

/// How long head reads wait for siblings before going out together.
const laneHeadBatchWindow = Duration(milliseconds: 50);

/// At most this many lanes per read (the relay's per-list limit is far
/// higher; ten keeps one read small).
const laneHeadBatchMax = 10;

/// The lane `d` of a head read: `{30078, authors:[one], #d:[one], limit:1}`
/// and nothing else. Anything else gets null and is read on its own.
String? laneHeadDTagOf(NostrFilter filter) {
  final d = filter.tags['#d'];
  if (filter.kinds.length != 1 ||
      filter.kinds.single != _laneKind ||
      filter.authors?.length != 1 ||
      filter.limit != 1 ||
      d == null ||
      d.length != 1 ||
      filter.tags.length != 1 ||
      filter.ids != null ||
      filter.since != null ||
      filter.until != null ||
      filter.search != null ||
      filter.extensions.isNotEmpty) {
    return null;
  }
  return d.single;
}

/// The retained head order: newest first, lower id first at equal second.
bool _isNewer(NostrEvent a, NostrEvent b) =>
    a.createdAt > b.createdAt ||
    (a.createdAt == b.createdAt && a.id.compareTo(b.id) < 0);

class _Waiter {
  _Waiter(this.dTag, this.timeout);

  final String dTag;
  final Duration timeout;
  final completer = Completer<List<NostrEvent>>();
}

/// Coalesces lane head reads by the same author that start within
/// [laneHeadBatchWindow] into one one-shot
/// `{30078, authors:[me], #d:[lanes], limit:N}`; each caller gets the newest
/// event of its own lane, as its single-lane read would have returned
/// (docs/message-sync.md#launch-order).
class LaneHeadBatcher {
  LaneHeadBatcher({
    required Future<List<NostrEvent>> Function(
      NostrFilter filter,
      Duration timeout,
    )
    fetch,
    Timer Function(Duration duration, void Function() callback)? timer,
  }) : _fetch = fetch,
       _timer = timer ?? Timer.new;

  final Future<List<NostrEvent>> Function(NostrFilter, Duration) _fetch;
  final Timer Function(Duration, void Function()) _timer;
  final _pending = <String, List<_Waiter>>{};
  final _timers = <String, Timer>{};

  /// Read the head of [filter]'s lane (see [laneHeadDTagOf]).
  Future<List<NostrEvent>> fetch(NostrFilter filter, Duration timeout) {
    final dTag = laneHeadDTagOf(filter);
    if (dTag == null) throw ArgumentError('Not a lane head read.');
    final author = filter.authors!.single;
    final waiter = _Waiter(dTag, timeout);
    final waiters = _pending.putIfAbsent(author, () => []);
    waiters.add(waiter);
    if (waiters.map((w) => w.dTag).toSet().length >= laneHeadBatchMax) {
      _flush(author);
    } else {
      _timers[author] ??= _timer(laneHeadBatchWindow, () => _flush(author));
    }
    return waiter.completer.future;
  }

  void _flush(String author) {
    _timers.remove(author)?.cancel();
    final waiters = _pending.remove(author);
    if (waiters == null || waiters.isEmpty) return;
    final lanes = {for (final waiter in waiters) waiter.dTag}.toList()..sort();
    final timeout = waiters
        .map((waiter) => waiter.timeout)
        .reduce((a, b) => a < b ? a : b);
    _fetch(
      NostrFilter(
        kinds: const [_laneKind],
        authors: [author],
        tags: {'#d': lanes},
        limit: lanes.length,
      ),
      timeout,
    ).then(
      (events) {
        final heads = <String, NostrEvent>{};
        for (final event in events) {
          if (event.pubkey != author || event.kind != _laneKind) continue;
          final d = event.getTagValue('d');
          if (d == null) continue;
          final current = heads[d];
          if (current == null || _isNewer(event, current)) heads[d] = event;
        }
        for (final waiter in waiters) {
          final head = heads[waiter.dTag];
          waiter.completer.complete(head == null ? const [] : [head]);
        }
      },
      onError: (Object error, StackTrace stack) {
        for (final waiter in waiters) {
          waiter.completer.completeError(error, stack);
        }
      },
    );
  }

  /// Fail every waiting read (session teardown).
  void reset(Object error) {
    for (final timer in _timers.values) {
      timer.cancel();
    }
    _timers.clear();
    final waiters = [for (final list in _pending.values) ...list];
    _pending.clear();
    for (final waiter in waiters) {
      waiter.completer.completeError(error);
    }
  }
}
