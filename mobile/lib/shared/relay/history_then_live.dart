import 'nostr_models.dart';

const _seenLimit = 2000;

/// A listener opened by [historyThenLive], with its history fetch.
class HistoryThenLive {
  HistoryThenLive._(this._state, this._stopLive, this.firstFetch);

  final _GateState _state;
  final void Function() _stopLive;

  /// Whether the first fetch succeeded (false after a failure or [stop]).
  final Future<bool> firstFetch;

  /// Fetch again through the same gate; completes with whether it succeeded.
  Future<bool> refetch() => _state.fetchOnce();

  void stop() {
    if (_state.stopped) return;
    _state.stopped = true;
    _stopLive();
  }
}

class _GateState {
  _GateState(this._fetch, this._apply, this._onFetchError);

  final Future<List<NostrEvent>> Function() _fetch;
  final void Function(NostrEvent) _apply;
  final void Function(Object error)? _onFetchError;
  final _seen = <String>{};
  bool stopped = false;

  void gate(NostrEvent event) {
    if (stopped || !_seen.add(event.id)) return;
    if (_seen.length > _seenLimit) _seen.remove(_seen.first);
    _apply(event);
  }

  Future<bool> fetchOnce() async {
    if (stopped) return false;
    final List<NostrEvent> events;
    try {
      events = await _fetch();
    } catch (error) {
      if (!stopped) _onFetchError?.call(error);
      return false;
    }
    if (stopped) return false;
    for (final event in [
      ...events,
    ]..sort((a, b) => a.createdAt.compareTo(b.createdAt))) {
      gate(event);
    }
    return true;
  }
}

/// Listen first, then fetch history once, so nothing published in between
/// is lost. Live and fetched events share one `seen` gate, so an event from
/// both sources reaches [apply] once; fetched events go oldest first. A
/// [HistoryThenLive.stop] during a fetch drops its result, and a failed fetch
/// goes to [onFetchError], never treated as an empty result. Throws, with
/// nothing registered, if [listen] fails.
Future<HistoryThenLive> historyThenLive({
  required Future<void Function()> Function(void Function(NostrEvent) apply)
  listen,
  required Future<List<NostrEvent>> Function() fetch,
  required void Function(NostrEvent) apply,
  void Function(Object error)? onFetchError,
}) async {
  final state = _GateState(fetch, apply, onFetchError);
  final stopLive = await listen(state.gate);
  return HistoryThenLive._(state, stopLive, state.fetchOnce());
}
