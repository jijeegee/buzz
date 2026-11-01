part of 'relay_session.dart';

/// The relay's filters-per-REQ limit.
const maxLiveFilters = 10;

/// Bundle filters are live only: `limit: 0`, no `since`/`until`, no room
/// (`#h`) and no search, at most ten. Replay re-sends them unchanged.
void assertBundleLiveFilters(List<NostrFilter> filters) {
  if (filters.isEmpty || filters.length > maxLiveFilters) {
    throw ArgumentError('A live bundle REQ needs 1-$maxLiveFilters filters.');
  }
  for (final filter in filters) {
    if (filter.limit != 0 ||
        filter.since != null ||
        filter.until != null ||
        filter.search != null ||
        filter.tags.containsKey('#h')) {
      throw ArgumentError('A live bundle filter must be limit 0, no history.');
    }
  }
}

/// A bundle's multi-filter REQ (see [RelaySessionLiveFilters]).
class _LiveMulti {
  _LiveMulti(this.filters);

  List<NostrFilter> filters;

  /// A REQ for this subId awaits its EOSE (or a CLOSED retry is pending).
  bool inFlight = false;

  /// [filters] changed since the last REQ was built.
  bool dirty = false;

  /// The REQ in flight came from [RelaySessionLiveFilters._sendFiltersUpdate];
  /// its EOSE is not a "ready" for listeners.
  bool updateInFlight = false;
  bool closed = false;
  List<Completer<void>> sentWaiters = [];
  List<Completer<void>> heldWaiters = [];

  void sent({required bool update}) {
    inFlight = true;
    dirty = false;
    updateInFlight = update;
    sentWaiters.addAll(heldWaiters);
    heldWaiters = [];
  }

  /// The relay answered the REQ in flight. Returns whether it was an update.
  bool settled() {
    final wasUpdate = updateInFlight;
    inFlight = false;
    updateInFlight = false;
    final waiters = sentWaiters;
    sentWaiters = [];
    for (final waiter in waiters) {
      if (!waiter.isCompleted) waiter.complete();
    }
    return wasUpdate;
  }

  /// A CLOSED retry is next in flight and carries the newest set.
  void lost() {
    inFlight = true;
    updateInFlight = false;
    heldWaiters = [...sentWaiters, ...heldWaiters];
    sentWaiters = [];
  }

  void releaseAll() {
    closed = true;
    for (final waiter in [...sentWaiters, ...heldWaiters]) {
      if (!waiter.isCompleted) waiter.complete();
    }
    sentWaiters = [];
    heldWaiters = [];
  }
}

bool _sameFilters(List<NostrFilter> left, List<NostrFilter> right) =>
    jsonEncode([for (final f in left) f.toJson()]) ==
    jsonEncode([for (final f in right) f.toJson()]);

/// One live REQ with several filters, for a bundle. At most one REQ is in
/// flight for it: the opening REQ, a reconnect replay or a CLOSED retry
/// counts until its EOSE. `setFilters` meanwhile only updates the entry, and
/// the newest set goes out after that EOSE (the relay may apply two REQs on
/// one subId out of order). The EOSE answering a `setFilters` REQ is not a
/// "ready" for [onStatusChanged]: only the opening, replay and retry ones are.
extension RelaySessionLiveFilters on RelaySessionNotifier {
  Future<LiveFiltersHandle> _subscribeFilters(
    List<NostrFilter> filters,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
    void Function(RelaySubscriptionStatus status)? onStatusChanged,
  }) async {
    assertBundleLiveFilters(filters);
    final multi = _LiveMulti(List.unmodifiable(filters));
    String? subId;
    final unsubscribe = await _subscribe(
      filters.first,
      onEvent,
      onClosed: (message) {
        multi.releaseAll();
        onClosed?.call(message);
      },
      onStatusChanged: onStatusChanged,
      multi: multi,
      onSubId: (id) => subId = id,
    );
    return LiveFiltersHandle(
      close: () {
        multi.releaseAll();
        unsubscribe();
      },
      setFilters: (next) {
        assertBundleLiveFilters(next);
        final id = subId;
        if (multi.closed || id == null || _sameFilters(multi.filters, next)) {
          return Future.value();
        }
        multi.filters = List.unmodifiable(next);
        multi.dirty = true;
        final subscription = _liveSubscriptions[id];
        if (subscription == null || subscription.multi != multi) {
          return Future.value();
        }
        subscription.filter = next.first;
        final waiter = Completer<void>();
        multi.heldWaiters.add(waiter);
        _sendFiltersUpdate(id, subscription);
        return waiter.future;
      },
    );
  }

  /// The REQ frame for a live entry: every filter of a multi-filter one.
  void _sendLiveReq(
    String subId,
    _LiveSubscription subscription,
    NostrFilter filter, {
    bool update = false,
  }) {
    final multi = subscription.multi;
    if (multi == null) {
      _sendReq(subId, filter);
      return;
    }
    multi.sent(update: update);
    _socket?.send([
      'REQ',
      subId,
      for (final each in multi.filters) each.toJson(),
    ]);
  }

  /// Send the newest set now unless a REQ is in flight or the socket is
  /// down (the replay carries it then).
  void _sendFiltersUpdate(String subId, _LiveSubscription subscription) {
    final multi = subscription.multi;
    if (multi == null || multi.closed || !multi.dirty || multi.inFlight) {
      return;
    }
    if (!_socketConnected || _socket == null) return;
    if (_rateLimitGate.isActive) {
      final generation = _connectionGeneration;
      unawaited(
        _rateLimitGate.wait().then((_) {
          if (!_isActiveConnection(generation) ||
              _liveSubscriptions[subId] != subscription) {
            return;
          }
          _sendFiltersUpdate(subId, subscription);
        }),
      );
      return;
    }
    _sendLiveReq(subId, subscription, subscription.filter, update: true);
  }

  /// EOSE on a live entry. Returns whether listeners hear "ready".
  bool _settleLiveReq(String subId, _LiveSubscription subscription) {
    final multi = subscription.multi;
    if (multi == null) return true;
    final wasUpdate = multi.settled();
    _sendFiltersUpdate(subId, subscription);
    return !wasUpdate;
  }
}
