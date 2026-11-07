/// Store-first windows checked against the relay: a room or thread window
/// drawn from the device store is compared with the relay's oldest row id
/// and row count for the same page query (bridge `/query` with
/// `"check": true`, answered by the bounds event alone). Equal: the store's
/// window stands and no page is fetched (mode `on`). Different: the page is
/// fetched as before and replaces the stored rows of its key range.
library;

import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';

import 'nostr_models.dart';

/// How windows open: `off` as before, `shadow` checks and still fetches
/// every page (logging both), `on` skips the page when the check is equal.
enum WindowCheckMode { off, shadow, on }

/// The developer override; `follow` takes the relay's mode.
enum WindowCheckOverride { follow, off, shadow, on }

/// Saved developer override (device-local).
const windowCheckOverrideKey = 'kuzz_mobile_window_check_override';

WindowCheckOverride windowCheckOverrideOf(String? name) =>
    WindowCheckOverride.values
        .where((value) => value.name == name)
        .firstOrNull ??
    WindowCheckOverride.follow;

/// After an `off` answer the relay is asked again this much later, and a
/// window whose refetch did not settle it opens as before for this long.
const windowCheckRetry = Duration(minutes: 10);

const _threadBoundsKind = 39007;

/// The relay's answer to a check.
@immutable
class WindowCheckAnswer {
  const WindowCheckAnswer({
    required this.oldestId,
    required this.count,
    required this.mode,
    this.oldestCreatedAt,
    this.roomSeq,
  });

  final String? oldestId;
  final int? oldestCreatedAt;
  final int count;
  final int? roomSeq;
  final WindowCheckMode mode;
}

/// The `check` of the first bounds event (39006 / 39007) in [events], or
/// `null` when the relay answered without one (it does not know `check`).
WindowCheckAnswer? parseWindowCheckAnswer(List<NostrEvent> events) {
  for (final event in events) {
    if (event.kind != EventKind.channelWindowBounds &&
        event.kind != _threadBoundsKind) {
      continue;
    }
    final Object? content;
    try {
      content = jsonDecode(event.content);
    } on FormatException {
      return null;
    }
    if (content is! Map) return null;
    final check = content['check'];
    if (check is! Map) return null;
    final count = check['count'];
    final oldest = check['oldest_id'];
    final mode = WindowCheckMode.values
        .where((value) => value.name == check['mode'])
        .firstOrNull;
    if (count is! int || mode == null) return null;
    if (oldest != null && oldest is! String) return null;
    final at = check['oldest_created_at'];
    final seq = check['room_seq'];
    return WindowCheckAnswer(
      oldestId: oldest as String?,
      oldestCreatedAt: at is int ? at : null,
      count: count,
      roomSeq: seq is int ? seq : null,
      mode: mode,
    );
  }
  return null;
}

/// [page] asked as a check: same filter, plus `"check": true`.
NostrFilter windowCheckFilter(NostrFilter page) => NostrFilter(
  kinds: page.kinds,
  authors: page.authors,
  ids: page.ids,
  limit: page.limit,
  since: page.since,
  until: page.until,
  search: page.search,
  tags: page.tags,
  extensions: {...page.extensions, 'check': true},
);

/// What the check compares: a window's oldest row id and row count.
typedef WindowFacts = ({String? oldestId, int count});

/// A relay page as the check sees it: its rows of [kinds] before the bounds
/// event (newest first, so the oldest is the last), and whether it has more.
typedef PageWindow = ({
  WindowFacts facts,
  (int, String)? oldest,
  Set<String> ids,
  bool hasMore,
  bool bounded,
});

PageWindow pageWindowFacts(List<NostrEvent> page, List<int> kinds) {
  NostrEvent? oldest;
  final ids = <String>{};
  var hasMore = false;
  var bounded = false;
  for (final event in page) {
    if (event.kind == EventKind.channelWindowBounds ||
        event.kind == _threadBoundsKind) {
      bounded = true;
      try {
        final content = jsonDecode(event.content);
        hasMore = content is Map && content['has_more'] == true;
      } on FormatException {
        hasMore = false;
      }
      break;
    }
    if (!kinds.contains(event.kind)) continue;
    oldest = event;
    ids.add(event.id);
  }
  return (
    facts: (oldestId: oldest?.id, count: ids.length),
    oldest: oldest == null ? null : (oldest.createdAt, oldest.id),
    ids: ids,
    hasMore: hasMore,
    bounded: bounded,
  );
}

bool _sameFacts(WindowFacts stored, WindowCheckAnswer answer) =>
    stored.count == answer.count && stored.oldestId == answer.oldestId;

/// A window as the store holds it for the check.
class CheckedStoreWindow {
  const CheckedStoreWindow({required this.facts, required this.response});

  final WindowFacts facts;

  /// The window in the shape of the relay's page, with [hasMore] from the
  /// relay's count.
  final List<NostrEvent> Function({required bool hasMore}) response;
}

/// Per relay session (account and server): the relay's latest mode, the
/// developer override and the windows held to the old path.
class WindowCheckGate {
  WindowCheckGate({DateTime Function()? now, this.override})
    : _now = now ?? DateTime.now;

  final DateTime Function() _now;

  /// Read at each decision; `null` means follow.
  WindowCheckOverride Function()? override;

  WindowCheckMode? _serverMode;
  DateTime? _serverModeAt;
  bool _unsupported = false;
  final Map<String, DateTime> _held = {};

  WindowCheckOverride get _override =>
      override?.call() ?? WindowCheckOverride.follow;

  bool get _waiting =>
      (_serverMode == WindowCheckMode.off || _unsupported) &&
      _now().difference(_serverModeAt!) < windowCheckRetry;

  /// How the window [key] opens now: `null` as before (no check), `shadow`
  /// (check beside the page, also while the relay's mode is unknown) or
  /// `on` (store first, check after).
  WindowCheckMode? before(String key) {
    final override = _override;
    if (override == WindowCheckOverride.off) return null;
    final held = _held[key];
    if (held != null) {
      if (_now().isBefore(held)) return null;
      _held.remove(key);
    }
    // A relay that does not know the check is not asked again for a while,
    // whatever the override.
    if (_unsupported && _waiting) return null;
    switch (override) {
      case WindowCheckOverride.shadow:
        return WindowCheckMode.shadow;
      case WindowCheckOverride.on:
        return WindowCheckMode.on;
      case WindowCheckOverride.off:
      case WindowCheckOverride.follow:
        break;
    }
    final server = _serverMode;
    if (server == null) return WindowCheckMode.shadow;
    if (server == WindowCheckMode.off) {
      return _waiting ? null : WindowCheckMode.shadow;
    }
    return server;
  }

  /// Notes the relay's mode from [answer] (`null`: the relay does not know
  /// the check, or failed it; taken as `off`) and returns the mode in force.
  WindowCheckMode noteAnswer(WindowCheckAnswer? answer) {
    _serverMode = answer?.mode ?? WindowCheckMode.off;
    _unsupported = answer == null;
    _serverModeAt = _now();
    if (answer == null) return WindowCheckMode.off;
    return switch (_override) {
      WindowCheckOverride.follow => _serverMode!,
      WindowCheckOverride.off => WindowCheckMode.off,
      WindowCheckOverride.shadow => WindowCheckMode.shadow,
      WindowCheckOverride.on => WindowCheckMode.on,
    };
  }

  /// A refetch did not settle [key]: it opens as before for a while.
  void hold(String key) => _held[key] = _now().add(windowCheckRetry);

  /// Whether the check is known to be in use: by the override, or by the
  /// relay's latest answer.
  bool get active {
    if (_unsupported) return false;
    return switch (_override) {
      WindowCheckOverride.follow =>
        _serverMode == WindowCheckMode.shadow ||
            _serverMode == WindowCheckMode.on,
      WindowCheckOverride.off => false,
      WindowCheckOverride.shadow || WindowCheckOverride.on => true,
    };
  }

  bool isHeld(String key) {
    final held = _held[key];
    return held != null && _now().isBefore(held);
  }
}

/// A `/query` for a window's page (or its fallbacks).
typedef WindowQuery =
    Future<List<NostrEvent>> Function(List<NostrFilter> filters);

/// A window opening through the check.
class CheckedWindowOpen {
  const CheckedWindowOpen._({required this.stored, required this.query});

  /// Mode `on`: the store's window, to draw at once (before the check
  /// answers); resolves `null` when the store cannot stand for the page.
  /// `null` in shadow: the window draws as before.
  final Future<List<NostrEvent>?>? stored;

  /// Stands in for the plain `/query` of the window's page. Its first call
  /// is the page: in mode `on` it checks the stored window and answers with
  /// it again when equal (no page fetched), else fetches the page; in
  /// shadow it fetches the page and checks beside it. Later calls (a
  /// thread window's narrower fallbacks) are plain fetches.
  final WindowQuery query;
}

/// Replaces the stored rows of a fetched [page]'s key range, then reads the
/// stored window again (for the loop guard); `null` when it cannot. A
/// `null` page only releases the store mark taken for it.
typedef WindowReplace = Future<WindowFacts?> Function(List<NostrEvent>? page);

/// The one open path of a room or thread window ([key]; [label] for logs),
/// or `null` when it opens as before (mode off, or held).
///
/// [readStored] answers `null` when the window is not fully stored.
/// [check] asks the relay's check for the page filter it is given (the one
/// actually sent) and returns the raw answer. [beginReplace] is called
/// before any page goes out (it marks the store, so rows written after
/// are never taken for deleted ones). [liveRows] counts live rows received
/// for the room: a difference while rows arrived is checked once more
/// before the page is fetched. [fetch] is the plain page query.
CheckedWindowOpen? openWindowCheck({
  required WindowCheckGate gate,
  required String key,
  required String label,
  required List<int> kinds,
  required int limit,
  required Future<CheckedStoreWindow?> Function() readStored,
  required Future<List<NostrEvent>> Function(NostrFilter page) check,
  required WindowReplace Function() beginReplace,
  required int Function() liveRows,
  required WindowQuery fetch,
  void Function(String message)? log,
}) {
  final pre = gate.before(key);
  if (pre == null) return null;
  void note(String message) =>
      (log ?? debugPrint)('[WindowCheck] $label $message');
  final clock = Stopwatch()..start();

  /// The relay's answer, and its events: a relay that does not know the
  /// check answers the page itself. Failures (a thread window's parser
  /// rejecting `check`) are `null`: the window opens as before.
  Future<(WindowCheckAnswer?, List<NostrEvent>?)> ask(NostrFilter page) =>
      check(page).then(
        (events) => (parseWindowCheckAnswer(events), events),
        onError: (Object error) {
          note('check failed: $error');
          return (null, null);
        },
      );
  Future<CheckedStoreWindow?> read() => readStored().then<CheckedStoreWindow?>(
    (stored) => stored,
    onError: (_) => null,
  );

  /// Replaces the page's range; still different after: held to the old path.
  Future<void> settle(WindowReplace replace, List<NostrEvent> page) async {
    final fetched = pageWindowFacts(page, kinds);
    final stored = await replace(page);
    if (stored == null) return;
    if (stored != fetched.facts) {
      gate.hold(key);
      note(
        'still different after refetch (stored ${stored.count} '
        '${stored.oldestId}, page ${fetched.facts.count} '
        '${fetched.facts.oldestId}): old path for ${windowCheckRetry.inMinutes} min',
      );
    }
  }

  /// Fetches the page under a fresh store mark, then [after] it.
  Future<List<NostrEvent>> fetchThen(
    List<NostrFilter> filters,
    WindowReplace replace,
    Future<void> Function(List<NostrEvent> page) after,
  ) async {
    final List<NostrEvent> page;
    try {
      page = await fetch(filters);
    } catch (_) {
      unawaited(replace(null).catchError((_) => null));
      rethrow;
    }
    unawaited(
      after(page).catchError((Object error) => note('after page: $error')),
    );
    return page;
  }

  var asked = false;

  if (pre == WindowCheckMode.shadow) {
    // Beside the page: nothing waits for the check.
    final replace = beginReplace();
    final stored = read();
    return CheckedWindowOpen._(
      stored: null,
      query: (filters) {
        if (asked || filters.length != 1) return fetch(filters);
        asked = true;
        Future<(WindowCheckAnswer?, List<NostrEvent>?)?> asking() async {
          final window = await stored;
          return window == null ? null : ask(filters.single);
        }

        final answer = asking();
        return fetchThen(filters, replace, (page) async {
          final pageMs = clock.elapsedMilliseconds;
          final window = await stored;
          if (window == null) {
            // Only while the check is known to be in use: a probe of a
            // relay that may be `off` changes nothing.
            await replace(gate.active ? page : null);
            return;
          }
          final (result, _) = (await answer)!;
          final mode = gate.noteAnswer(result);
          if (result == null || mode == WindowCheckMode.off) {
            await replace(null);
            return;
          }
          final equal = _sameFacts(window.facts, result);
          note(
            'shadow ${equal ? 'equal' : 'different'} (check '
            '${clock.elapsedMilliseconds} ms, page $pageMs ms)',
          );
          equal ? await replace(null) : await settle(replace, page);
        });
      },
    );
  }

  // Mode on: the store's window draws at once; the check follows.
  final window = read();
  return CheckedWindowOpen._(
    stored: window.then(
      (window) => window?.response(hasMore: window.facts.count >= limit),
    ),
    query: (filters) async {
      if (asked || filters.length != 1) return fetch(filters);
      asked = true;
      var stored = await window;
      if (stored == null) {
        note('not fully stored: page fetched');
        final replace = beginReplace();
        return fetchThen(filters, replace, (page) async {
          await replace(page);
        });
      }
      final live = liveRows();
      var (answer, events) = await ask(filters.single);
      var mode = gate.noteAnswer(answer);
      if (answer == null) {
        // A relay without the check answered the page itself.
        final page = events;
        if (page != null && pageWindowFacts(page, kinds).bounded) return page;
        return fetch(filters);
      }
      if (mode == WindowCheckMode.off) return fetch(filters);
      var equal = _sameFacts(stored.facts, answer);
      if (!equal && liveRows() != live) {
        // Rows arrived while the check was out: ask once more.
        final again = await read();
        if (again != null) {
          final (second, _) = await ask(filters.single);
          if (second != null) {
            stored = again;
            answer = second;
            mode = gate.noteAnswer(second);
            if (mode == WindowCheckMode.off) return fetch(filters);
            equal = _sameFacts(again.facts, second);
          }
        }
      }
      final checkMs = clock.elapsedMilliseconds;
      if (equal && mode == WindowCheckMode.on) {
        note('equal: the stored window stands ($checkMs ms)');
        return stored.response(hasMore: answer.count >= limit);
      }
      if (equal) {
        final page = await fetch(filters);
        note(
          'shadow equal (check $checkMs ms, page '
          '${clock.elapsedMilliseconds} ms)',
        );
        return page;
      }
      note(
        'different (stored ${stored.facts.count} ${stored.facts.oldestId}, '
        'relay ${answer.count} ${answer.oldestId}): page fetched',
      );
      final replace = beginReplace();
      return fetchThen(filters, replace, (page) => settle(replace, page));
    },
  );
}
