import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';

import 'me_live_bundle.dart' show eventMatchesFilter;
import 'nostr_models.dart';
import 'relay_session_types.dart';

/// Community-wide live filters one bundle REQ carries. Shared with desktop
/// through `test-fixtures/community-live-bundle.json`
/// (docs/message-sync.md#community-bundle).
enum CommunityRow { emoji, status, catalogs, presence }

const communityRowFilters = <CommunityRow, NostrFilter>{
  CommunityRow.emoji: NostrFilter(
    kinds: [30030],
    tags: {
      '#d': ['kuzz:custom-emoji'],
    },
    limit: 0,
  ),
  CommunityRow.status: NostrFilter(
    kinds: [30315],
    tags: {
      '#d': ['general'],
    },
    limit: 0,
  ),
  CommunityRow.catalogs: NostrFilter(kinds: [30175, 30178], limit: 0),
  // `authors` is the bundle's presence author set; omitted while empty.
  CommunityRow.presence: NostrFilter(kinds: [20001], limit: 0),
};

/// Mobile has no live emoji or catalog listeners.
const mobileCommunityRows = <CommunityRow>[
  CommunityRow.status,
  CommunityRow.presence,
];

/// The relay's values-per-list limit.
const presenceAuthorCap = 1000;
const presenceUpdateDebounce = Duration(milliseconds: 250);

/// At most one presence re-REQ per this long.
const presenceUpdateInterval = Duration(seconds: 30);
const communityOpenDelay = Duration(milliseconds: 250);
const _reopenBaseMs = 1000;
const _reopenMaxMs = 60000;
const _listenerSeenLimit = 2000;

bool _sameTag(List<String>? left, List<String>? right) {
  if (left == null || right == null) return left == right;
  return listEquals([...left]..sort(), [...right]..sort());
}

/// The row a listener filter rides: live only (`limit: 0`), kinds a subset
/// of one row's and the same `#d`. A filter with `authors` (it would silently
/// get only the tracked set), any other key, or history gets null.
CommunityRow? communityScopeOf(
  NostrFilter filter, [
  List<CommunityRow> rows = mobileCommunityRows,
]) {
  if (filter.limit != 0 ||
      filter.authors != null ||
      filter.ids != null ||
      filter.since != null ||
      filter.until != null ||
      filter.search != null ||
      filter.extensions.isNotEmpty ||
      filter.tags.keys.any((key) => key != '#d') ||
      filter.kinds.isEmpty) {
    return null;
  }
  for (final row in rows) {
    final rowFilter = communityRowFilters[row]!;
    if (!filter.kinds.every(rowFilter.kinds.contains)) continue;
    if (!_sameTag(filter.tags['#d'], rowFilter.tags['#d'])) continue;
    return row;
  }
  return null;
}

typedef CommunityBundleOpen =
    Future<LiveFiltersHandle> Function(
      List<NostrFilter> filters,
      void Function(NostrEvent) onEvent,
      void Function(RelaySubscriptionStatus status) onStatus,
      void Function() onGone,
    );

typedef CommunityTimerFactory =
    Timer Function(Duration duration, void Function() callback);

class _Listener {
  _Listener(this.filter, this.onEvent, this.onStatus, this.onReset);

  final NostrFilter filter;
  final void Function(NostrEvent) onEvent;
  final void Function(RelaySubscriptionStatus status)? onStatus;
  final void Function()? onReset;
  final seen = <String>{};

  void deliver(NostrEvent event) {
    if (seen.contains(event.id) || !eventMatchesFilter(event, filter)) return;
    seen.add(event.id);
    if (seen.length > _listenerSeenLimit) seen.remove(seen.first);
    onEvent(event);
  }
}

enum _State { closed, scheduled, opening, open, backoff }

/// Every community-wide live subscription on one REQ: the status row plus
/// presence for a growing author set (desktop also carries emoji and the
/// catalogs). Listeners keep their filters and get the events they match,
/// once.
///
/// - Opens 250 ms after its first listener and closes one task after its
///   last.
/// - The author set lives for the session: lowercased, sorted, grow-only,
///   capped at 1000 (past the cap keys are refused, with one warning, and the
///   caller keeps polling them). New keys go out 250 ms after the first one,
///   but at most one re-REQ per 30 s; the session sends it only after the
///   previous REQ's EOSE, and its EOSE is no "ready" for listeners.
/// - When the session drops the REQ (terminal CLOSED) or opening fails, the
///   bundle reopens with backoff (1 s up to 60 s); listeners stay and hear
///   "retrying", then "ready".
/// - [reset] (session teardown, config change) clears the set and tells
///   listeners to add their keys again.
class CommunityLiveBundle {
  CommunityLiveBundle({
    required CommunityBundleOpen open,
    List<CommunityRow> rows = mobileCommunityRows,
    CommunityTimerFactory? timer,
    int Function()? nowMs,
  }) : _open = open,
       _rows = rows,
       _timer = timer ?? Timer.new,
       _nowMs = nowMs ?? (() => DateTime.now().millisecondsSinceEpoch);

  final CommunityBundleOpen _open;
  final List<CommunityRow> _rows;
  final CommunityTimerFactory _timer;
  final int Function() _nowMs;
  final _listeners = <_Listener>{};
  final _authors = <String>{};
  var _capWarned = false;
  LiveFiltersHandle? _handle;
  var _state = _State.closed;
  var _generation = 0;
  Timer? _openTimer;
  Timer? _reopenTimer;
  Timer? _updateTimer;
  var _reopenAttempt = 0;
  int? _lastUpdateAt;
  var _sentAuthors = '';
  var _openWaiters = <Completer<void>>[];

  /// Listen with [filter] (see [communityScopeOf]). Completes with the stop
  /// function once the first open attempt settled; a failed one keeps the
  /// listener and retries.
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(RelaySubscriptionStatus status)? onStatus,
    void Function()? onReset,
  }) async {
    if (communityScopeOf(filter, _rows) == null) {
      throw ArgumentError('Filter is not a community bundle filter.');
    }
    final listener = _Listener(filter, onEvent, onStatus, onReset);
    _listeners.add(listener);
    var stopped = false;
    void stop() {
      if (stopped) return;
      stopped = true;
      _listeners.remove(listener);
      if (_listeners.isNotEmpty) return;
      scheduleMicrotask(() {
        if (_listeners.isEmpty) _close();
      });
    }

    if (_state == _State.closed) _scheduleOpen(communityOpenDelay);
    if (_state == _State.scheduled || _state == _State.opening) {
      final waiter = Completer<void>();
      _openWaiters.add(waiter);
      await waiter.future;
    }
    return stop;
  }

  /// Add people to the presence author set. Returns the keys past the cap,
  /// which the caller keeps polling.
  List<String> addPresenceAuthors(Iterable<String> pubkeys) {
    final refused = <String>[];
    var added = false;
    for (final raw in pubkeys) {
      final pubkey = raw.toLowerCase();
      if (_authors.contains(pubkey)) continue;
      if (_authors.length >= presenceAuthorCap) {
        refused.add(pubkey);
        continue;
      }
      _authors.add(pubkey);
      added = true;
    }
    if (refused.isNotEmpty && !_capWarned) {
      _capWarned = true;
      debugPrint(
        'Presence author set is full ($presenceAuthorCap); polling the rest.',
      );
    }
    if (added) _scheduleUpdate();
    return refused;
  }

  bool tracksPresenceOf(String pubkey) =>
      _authors.contains(pubkey.toLowerCase());

  /// The REQ's filters right now (the presence row only with authors).
  List<NostrFilter> currentFilters() => [
    for (final row in _rows)
      if (row != CommunityRow.presence)
        communityRowFilters[row]!
      else if (_authors.isNotEmpty)
        NostrFilter(
          kinds: communityRowFilters[row]!.kinds,
          authors: [..._authors]..sort(),
          limit: 0,
        ),
  ];

  /// Session reset: clear the author set and reopen for current listeners.
  void reset() {
    _close();
    _authors.clear();
    _capWarned = false;
    _lastUpdateAt = null;
    for (final listener in [..._listeners]) {
      listener.onReset?.call();
    }
    if (_listeners.isNotEmpty && _state == _State.closed) {
      _scheduleOpen(communityOpenDelay);
    }
  }

  String _authorsKey() => ([..._authors]..sort()).join(',');

  void _notify(RelaySubscriptionStatus status) {
    for (final listener in [..._listeners]) {
      listener.onStatus?.call(status);
    }
  }

  void _releaseOpenWaiters() {
    final waiters = _openWaiters;
    _openWaiters = [];
    for (final waiter in waiters) {
      if (!waiter.isCompleted) waiter.complete();
    }
  }

  void _scheduleOpen(Duration delay) {
    _state = _State.scheduled;
    final generation = _generation;
    _openTimer = _timer(delay, () {
      _openTimer = null;
      if (generation != _generation) return;
      if (_listeners.isEmpty) {
        _close();
        return;
      }
      _openNow();
    });
  }

  void _openNow() {
    final generation = ++_generation;
    _state = _State.opening;
    _sentAuthors = _authorsKey();
    bool isCurrent() => generation == _generation;
    _open(
      currentFilters(),
      (event) {
        if (!isCurrent()) return;
        for (final listener in [..._listeners]) {
          listener.deliver(event);
        }
      },
      (status) {
        if (isCurrent()) _notify(status);
      },
      () {
        if (isCurrent()) _backoff();
      },
    ).then(
      (handle) {
        if (!isCurrent()) {
          handle.close();
          return;
        }
        _handle = handle;
        _state = _State.open;
        _reopenAttempt = 0;
        _releaseOpenWaiters();
        if (_authorsKey() != _sentAuthors) _scheduleUpdate();
      },
      onError: (Object error) {
        if (!isCurrent()) return;
        debugPrint('Failed to open the community bundle: $error');
        _backoff();
        _releaseOpenWaiters();
      },
    );
  }

  void _backoff() {
    _dropRequest();
    if (_listeners.isEmpty) {
      _state = _State.closed;
      _releaseOpenWaiters();
      return;
    }
    _state = _State.backoff;
    _releaseOpenWaiters();
    _notify(RelaySubscriptionStatus.retrying);
    final delayMs = math.min(
      _reopenBaseMs * (1 << _reopenAttempt),
      _reopenMaxMs,
    );
    _reopenAttempt = math.min(_reopenAttempt + 1, 16);
    final generation = _generation;
    _reopenTimer = _timer(Duration(milliseconds: delayMs), () {
      _reopenTimer = null;
      if (generation != _generation) return;
      if (_listeners.isEmpty) {
        _state = _State.closed;
        return;
      }
      _openNow();
    });
  }

  void _scheduleUpdate() {
    if (_state != _State.open || _updateTimer != null) return;
    final last = _lastUpdateAt;
    final waitMs = last == null
        ? 0
        : last + presenceUpdateInterval.inMilliseconds - _nowMs();
    final delayMs = math.max(presenceUpdateDebounce.inMilliseconds, waitMs);
    _updateTimer = _timer(Duration(milliseconds: delayMs), () {
      _updateTimer = null;
      final handle = _handle;
      if (_state != _State.open || handle == null) return;
      final key = _authorsKey();
      if (key == _sentAuthors) return;
      _sentAuthors = key;
      _lastUpdateAt = _nowMs();
      unawaited(handle.setFilters(currentFilters()));
    });
  }

  /// Stop timers and the REQ; listeners and the author set stay.
  void _dropRequest() {
    _generation++;
    _openTimer?.cancel();
    _reopenTimer?.cancel();
    _updateTimer?.cancel();
    _openTimer = null;
    _reopenTimer = null;
    _updateTimer = null;
    final handle = _handle;
    _handle = null;
    handle?.close();
  }

  void _close() {
    _dropRequest();
    _state = _State.closed;
    _reopenAttempt = 0;
    _releaseOpenWaiters();
  }
}
