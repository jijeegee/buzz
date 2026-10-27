import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';

import 'nostr_models.dart';

/// Kinds the account's own documents and signals use. Shared with desktop
/// through `test-fixtures/me-live-bundle.json` (docs/message-sync.md).
const meAuthoredKinds = <int>[
  3,
  5,
  30078,
  30175,
  30176,
  30177,
  30178,
  30180,
  30181,
  30182,
  30300,
];

/// Kinds addressed to the account: mentions, approvals, reminders, chat-list
/// pings, sign-in and membership notices.
const meAddressedKinds = <int>[
  1,
  9,
  24210,
  24211,
  40002,
  40007,
  43001,
  43002,
  43003,
  43004,
  43005,
  43006,
  44100,
  44101,
  45001,
  45003,
  46010,
  46011,
  46012,
];

/// How far before the drop a reconnect catch-up starts: the relay's
/// created-at future tolerance (900s) plus its storage floor (960s) and a
/// margin, as desktop's reconnect repair.
const meCatchUpLookbackSeconds = 1865;

const _listenerTagKeys = {'#p', '#d', '#t'};
const _catchUpLimit = 500;
const _listenerSeenLimit = 2000;

const _catchUpAttempts = 3;
const _catchUpRetry = Duration(seconds: 1);

enum MeScopeKind { authored, addressed }

@immutable
class MeScope {
  const MeScope(this.kind, this.pubkey);

  final MeScopeKind kind;
  final String pubkey;

  String get key => '${kind.name}:$pubkey';

  NostrFilter get filter => kind == MeScopeKind.authored
      ? NostrFilter(kinds: meAuthoredKinds, authors: [pubkey], limit: 0)
      : NostrFilter(
          kinds: meAddressedKinds,
          tags: {
            '#p': [pubkey],
          },
          limit: 0,
        );

  @override
  bool operator ==(Object other) =>
      other is MeScope && other.kind == kind && other.pubkey == pubkey;

  @override
  int get hashCode => Object.hash(kind, pubkey);
}

/// The bundle [filter] can ride: live only (`limit: 0`, no `since`), one
/// pubkey in `authors` (authored) or in `#p` (addressed), never both, only
/// kinds that bundle carries, and only `#p`, `#d` and `#t` tags. Anything
/// else, including a filter that asks for history, returns null.
MeScope? meScopeOf(NostrFilter filter) {
  if (filter.limit != 0 ||
      filter.since != null ||
      filter.kinds.isEmpty ||
      filter.ids != null ||
      filter.until != null ||
      filter.search != null ||
      filter.extensions.isNotEmpty ||
      filter.tags.keys.any((key) => !_listenerTagKeys.contains(key))) {
    return null;
  }
  final authors = filter.authors;
  final addressed = filter.tags['#p'];
  if (authors != null && addressed == null && authors.length == 1) {
    if (!filter.kinds.every(meAuthoredKinds.contains)) return null;
    return MeScope(MeScopeKind.authored, authors.single.toLowerCase());
  }
  if (addressed != null && authors == null && addressed.length == 1) {
    if (!filter.kinds.every(meAddressedKinds.contains)) return null;
    return MeScope(MeScopeKind.addressed, addressed.single.toLowerCase());
  }
  return null;
}

/// Whether a live [event] reaches a listener with [filter].
bool eventMatchesFilter(NostrEvent event, NostrFilter filter) {
  if (!filter.kinds.contains(event.kind)) return false;
  final authors = filter.authors;
  if (authors != null &&
      !authors.any((a) => a.toLowerCase() == event.pubkey.toLowerCase())) {
    return false;
  }
  for (final MapEntry(key: key, value: values) in filter.tags.entries) {
    if (!key.startsWith('#')) continue;
    final name = key.substring(1);
    String norm(String value) => name == 'p' ? value.toLowerCase() : value;
    final wanted = values.map(norm).toSet();
    final hit = event.tags.any(
      (tag) =>
          tag.length > 1 && tag[0] == name && wanted.contains(norm(tag[1])),
    );
    if (!hit) return false;
  }
  return true;
}

typedef MeBundleOpen =
    Future<void Function()> Function(
      NostrFilter filter,
      void Function(NostrEvent) onEvent,
      void Function(String message) onClosed,
    );

typedef MeBundleFetch = Future<List<NostrEvent>> Function(NostrFilter filter);

class _Listener {
  _Listener(this.filter, this.onEvent, this.onClosed);

  final NostrFilter filter;
  final void Function(NostrEvent) onEvent;
  final void Function(String message)? onClosed;
  final seen = <String>{};

  void deliver(NostrEvent event) {
    if (seen.contains(event.id) || !eventMatchesFilter(event, filter)) return;
    seen.add(event.id);
    if (seen.length > _listenerSeenLimit) seen.remove(seen.first);
    onEvent(event);
  }
}

class _ScopeEntry {
  _ScopeEntry(this.scope);

  final MeScope scope;
  final listeners = <_Listener>{};
  late Future<void> ready;
  bool live = false;
  void Function()? stop;

  /// Start of a window a failed catch-up still owes; paid next reconnect.
  int? owedSince;
}

int _byCreatedAt(NostrEvent a, NostrEvent b) =>
    a.createdAt.compareTo(b.createdAt);

/// Every live subscription about the account on two REQs: what it authored
/// (`authors: [me]`) and what is addressed to it (`#p: [me]`). Features keep
/// their filters; the bundle opens one `limit: 0` REQ per scope with every
/// kind that scope carries and hands each listener the events its filter
/// matches, once. After a reconnect the bundle fetches what each scope missed
/// while the connection was down ([catchUp]).
///
/// A scope closes once its last listener leaves, after the current task, so
/// a provider rebuild that resubscribes at once keeps it.
class MeLiveBundle {
  MeLiveBundle({required MeBundleOpen open, required MeBundleFetch fetch})
    : _open = open,
      _fetch = fetch;

  final MeBundleOpen _open;
  final MeBundleFetch _fetch;
  final _scopes = <String, _ScopeEntry>{};
  var _generation = 0;

  /// Listen with [filter] (see [meScopeOf]). Completes once the scope's REQ
  /// is live, with the stop function; throws if it fails, leaving nothing
  /// registered.
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    final scope = meScopeOf(filter);
    if (scope == null) {
      throw ArgumentError('Filter is not scoped to one account.');
    }
    final entry = _scopes[scope.key] ?? _openScope(scope);
    final listener = _Listener(filter, onEvent, onClosed);
    entry.listeners.add(listener);

    var stopped = false;
    void stop() {
      if (stopped) return;
      stopped = true;
      entry.listeners.remove(listener);
      if (entry.listeners.isNotEmpty || _scopes[scope.key] != entry) return;
      scheduleMicrotask(() {
        if (entry.listeners.isNotEmpty || _scopes[scope.key] != entry) return;
        _scopes.remove(scope.key);
        _closeScope(entry);
      });
    }

    try {
      await entry.ready;
      return stop;
    } catch (_) {
      stop();
      rethrow;
    }
  }

  /// Fetch what every live scope missed since [sinceSecs] (the moment the
  /// connection dropped, minus the relay's created-at skew allowance). A
  /// scope whose fetch keeps failing owes that window to the next reconnect.
  Future<void> catchUp(int sinceSecs) async {
    final generation = _generation;
    await Future.wait([
      for (final entry in _scopes.values)
        if (entry.live) _catchUpScope(entry, sinceSecs, generation),
    ]);
  }

  /// Drop every scope (session teardown).
  void reset() {
    _generation++;
    for (final entry in _scopes.values) {
      _closeScope(entry);
    }
    _scopes.clear();
  }

  _ScopeEntry _openScope(MeScope scope) {
    final generation = _generation;
    final entry = _ScopeEntry(scope);
    bool isCurrent() =>
        generation == _generation && _scopes[scope.key] == entry;
    entry.ready =
        _open(
          scope.filter,
          (event) {
            if (!isCurrent()) return;
            for (final listener in [...entry.listeners]) {
              listener.deliver(event);
            }
          },
          (message) {
            // A terminal CLOSED: the next subscriber opens a fresh REQ, and each
            // listener reacts as it did to its own REQ closing.
            if (!isCurrent()) return;
            _scopes.remove(scope.key);
            _closeScope(entry);
            for (final listener in [...entry.listeners]) {
              listener.onClosed?.call(message);
            }
          },
        ).then(
          (stop) {
            if (!isCurrent()) {
              stop();
              return;
            }
            entry.stop = stop;
            entry.live = true;
          },
          onError: (Object error, StackTrace stackTrace) {
            if (_scopes[scope.key] == entry) _scopes.remove(scope.key);
            Error.throwWithStackTrace(error, stackTrace);
          },
        );
    // Callers observe the error; this keeps an unobserved one quiet.
    unawaited(entry.ready.catchError((_) {}));
    _scopes[scope.key] = entry;
    return entry;
  }

  void _closeScope(_ScopeEntry entry) {
    entry.live = false;
    entry.stop?.call();
    entry.stop = null;
  }

  Future<void> _catchUpScope(
    _ScopeEntry entry,
    int sinceSecs,
    int generation,
  ) async {
    bool isCurrent() =>
        generation == _generation && entry.live && entry.listeners.isNotEmpty;
    final since = math.max(
      0,
      math.min(sinceSecs, entry.owedSince ?? sinceSecs),
    );
    entry.owedSince = since;
    for (var attempt = 1; attempt <= _catchUpAttempts; attempt++) {
      if (!isCurrent()) return;
      try {
        final missed = await _fetch(
          _withLimit(entry.scope.filter.copyWithSince(since), _catchUpLimit),
        );
        if (!isCurrent()) return;
        entry.owedSince = null;
        for (final event in [...missed]..sort(_byCreatedAt)) {
          for (final listener in [...entry.listeners]) {
            listener.deliver(event);
          }
        }
        return;
      } catch (error) {
        if (attempt == _catchUpAttempts) {
          debugPrint('Failed to catch up account events: $error');
          return;
        }
        await Future<void>.delayed(_catchUpRetry * attempt);
      }
    }
  }
}

NostrFilter _withLimit(NostrFilter filter, int limit) => NostrFilter(
  kinds: filter.kinds,
  authors: filter.authors,
  ids: filter.ids,
  limit: limit,
  since: filter.since,
  until: filter.until,
  search: filter.search,
  tags: filter.tags,
  extensions: filter.extensions,
);
