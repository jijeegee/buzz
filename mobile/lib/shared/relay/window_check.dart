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
});

PageWindow pageWindowFacts(List<NostrEvent> page, List<int> kinds) {
  NostrEvent? oldest;
  final ids = <String>{};
  var hasMore = false;
  for (final event in page) {
    if (event.kind == EventKind.channelWindowBounds ||
        event.kind == _threadBoundsKind) {
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
  final Map<String, DateTime> _held = {};

  WindowCheckOverride get _override =>
      override?.call() ?? WindowCheckOverride.follow;

  /// How the window [key] opens now: `null` as before (no check), `shadow`
  /// (check beside the page, also while the relay's mode is unknown) or
  /// `on` (check first).
  WindowCheckMode? before(String key) {
    final override = _override;
    if (override == WindowCheckOverride.off) return null;
    final held = _held[key];
    if (held != null) {
      if (_now().isBefore(held)) return null;
      _held.remove(key);
    }
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
      final at = _serverModeAt!;
      return _now().difference(at) >= windowCheckRetry
          ? WindowCheckMode.shadow
          : null;
    }
    return server;
  }

  /// Notes the relay's mode from [answer] (`null`: the relay does not know
  /// the check, taken as `off`) and returns the mode in force.
  WindowCheckMode noteAnswer(WindowCheckAnswer? answer) {
    _serverMode = answer?.mode ?? WindowCheckMode.off;
    _serverModeAt = _now();
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
  bool get active => switch (_override) {
    WindowCheckOverride.follow =>
      _serverMode == WindowCheckMode.shadow ||
          _serverMode == WindowCheckMode.on,
    WindowCheckOverride.off => false,
    WindowCheckOverride.shadow || WindowCheckOverride.on => true,
  };

  bool isHeld(String key) {
    final held = _held[key];
    return held != null && _now().isBefore(held);
  }
}

/// What a window open does after the decision.
class WindowCheckDecision {
  const WindowCheckDecision._({this.stored, this.onPage});

  static const today = WindowCheckDecision._();

  /// The store's window, checked equal: open from it, fetch no page.
  final List<NostrEvent>? stored;

  /// Called with the page fetched as before (shadow logging, replacing the
  /// stored rows, the loop guard). Never throws.
  final Future<void> Function(List<NostrEvent> page)? onPage;
}

extension WindowCheckPage on WindowCheckDecision? {
  /// [page] (fetched as before), handed to [WindowCheckDecision.onPage] once
  /// it answers.
  Future<List<NostrEvent>> relayed(Future<List<NostrEvent>> page) {
    final onPage = this?.onPage;
    if (onPage == null) return page;
    return page.then((events) {
      unawaited(onPage(events));
      return events;
    });
  }
}

/// Replaces the stored rows of a fetched [page]'s key range, then reads the
/// stored window again (for the loop guard); `null` when it cannot. A
/// `null` page only releases the store mark taken for it.
typedef WindowReplace = Future<WindowFacts?> Function(List<NostrEvent>? page);

/// The one open path of a room or thread window ([key]; [label] for logs):
/// draw from the store, check it, and either stand on it or fetch the page
/// as before.
///
/// [readStored] answers `null` when the window is not fully stored.
/// [beginReplace] is called before any page goes out (it marks the store,
/// so rows written after are never taken for deleted ones). [liveRows]
/// counts live rows received for the room: a difference while rows arrived
/// is checked once more before the page is fetched.
Future<WindowCheckDecision> decideWindowCheck({
  required WindowCheckGate gate,
  required String key,
  required String label,
  required List<int> kinds,
  required int limit,
  required Future<CheckedStoreWindow?> Function() readStored,
  required Future<WindowCheckAnswer?> Function() check,
  required WindowReplace Function() beginReplace,
  required int Function() liveRows,
  void Function(String message)? log,
}) async {
  final pre = gate.before(key);
  if (pre == null) return WindowCheckDecision.today;
  void note(String message) =>
      (log ?? debugPrint)('[WindowCheck] $label $message');
  final clock = Stopwatch()..start();

  Future<WindowCheckAnswer?> ask() => check().then<WindowCheckAnswer?>(
    (answer) => answer,
    onError: (error) {
      note('check failed: $error');
      return null;
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

  if (pre == WindowCheckMode.shadow) {
    // Beside the page: nothing waits for the check.
    final replace = beginReplace();
    final stored = read();
    final answer = stored.then((window) => window == null ? null : ask());
    return WindowCheckDecision._(
      onPage: (page) async {
        try {
          final pageMs = clock.elapsedMilliseconds;
          final window = await stored;
          if (window == null) {
            // Only while the check is known to be in use: a probe of a
            // relay that may be `off` changes nothing.
            await replace(gate.active ? page : null);
            return;
          }
          final result = await answer;
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
        } catch (error) {
          note('shadow failed: $error');
        }
      },
    );
  }

  // Mode on: check first, fetch only when it differs.
  var window = await read();
  if (window == null) {
    note('not fully stored: page fetched');
    final replace = beginReplace();
    return WindowCheckDecision._(
      onPage: (page) async {
        try {
          await replace(page);
        } catch (error) {
          note('replace failed: $error');
        }
      },
    );
  }
  final live = liveRows();
  var answer = await ask();
  var mode = gate.noteAnswer(answer);
  if (answer == null || mode == WindowCheckMode.off) {
    return WindowCheckDecision.today;
  }
  var equal = _sameFacts(window.facts, answer);
  if (!equal && liveRows() != live) {
    // Rows arrived while the check was out: ask once more.
    final again = await read();
    final second = again == null ? null : await ask();
    if (again != null && second != null) {
      window = again;
      answer = second;
      mode = gate.noteAnswer(second);
      if (mode == WindowCheckMode.off) return WindowCheckDecision.today;
      equal = _sameFacts(again.facts, second);
    }
  }
  final checkMs = clock.elapsedMilliseconds;
  if (equal) {
    if (mode == WindowCheckMode.on) {
      note('equal: opened from the store ($checkMs ms)');
      return WindowCheckDecision._(
        stored: window.response(hasMore: answer.count >= limit),
      );
    }
    return WindowCheckDecision._(
      onPage: (_) async => note(
        'shadow equal (check $checkMs ms, page '
        '${clock.elapsedMilliseconds} ms)',
      ),
    );
  }
  note(
    'different (stored ${window.facts.count} ${window.facts.oldestId}, '
    'relay ${answer.count} ${answer.oldestId}): page fetched',
  );
  final replace = beginReplace();
  return WindowCheckDecision._(
    onPage: (page) async {
      try {
        await settle(replace, page);
      } catch (error) {
        note('refetch failed: $error');
      }
    },
  );
}
