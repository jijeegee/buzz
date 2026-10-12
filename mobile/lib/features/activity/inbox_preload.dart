import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/widgets.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../channels/channel_preload.dart';
import '../channels/thread_replies_provider.dart';
import 'inbox_item.dart';

/// Rows whose rooms load at once: about one screenful.
const inboxPreloadRows = 12;

/// Rows whose rooms load in the background after the first wave.
const inboxPreloadBackgroundRows = 48;

/// Input this recent means the user is scrolling or tapping: yield to them.
const _recentInput = Duration(milliseconds: 1500);
const _idlePoll = Duration(milliseconds: 500);

/// The room (and, for thread rows, the thread) behind one chat-list row.
@immutable
class InboxPreloadTarget {
  const InboxPreloadTarget({
    required this.channelId,
    required this.thread,
    required this.latestAt,
  });

  final String channelId;
  final ThreadRepliesArgs? thread;
  final int latestAt;

  String get key => thread == null ? channelId : '$channelId:${thread!.rootId}';
}

/// The rooms and threads behind the top chat-list rows, in display order.
/// Mirrors desktop's `inboxPreloadTargets`.
List<InboxPreloadTarget> inboxPreloadTargets(
  Iterable<InboxItem> rows, {
  int limit = inboxPreloadBackgroundRows,
}) {
  final targets = <InboxPreloadTarget>[];
  for (final row in rows.take(limit)) {
    final channelId = row.item.channelId;
    if (channelId == null) continue;
    final isRoom =
        row.conversationId.startsWith('channel:') ||
        row.conversationId.startsWith('dm:');
    final rootId = isRoom ? null : row.threadRootId;
    targets.add(
      InboxPreloadTarget(
        channelId: channelId,
        thread: rootId == null
            ? null
            : ThreadRepliesArgs(channelId: channelId, rootId: rootId),
        latestAt: row.latestActivityAt,
      ),
    );
  }
  return targets;
}

List<ChannelPreloadTarget> _rooms(Iterable<InboxPreloadTarget> targets) {
  final latestAt = <String, int>{};
  for (final target in targets) {
    latestAt[target.channelId] = math.max(
      latestAt[target.channelId] ?? 0,
      target.latestAt,
    );
  }
  return [
    for (final entry in latestAt.entries)
      (channelId: entry.key, latestAt: entry.value),
  ];
}

List<ThreadRepliesArgs> _threads(Iterable<InboxPreloadTarget> targets) => [
  for (final target in targets) ?target.thread,
];

/// Two waves, as desktop does: the rooms behind the top [inboxPreloadRows]
/// rows in one request and their threads four per request, right away; then
/// rows up to [inboxPreloadBackgroundRows], [maxChannelWindowsPerRequest]
/// rooms per request, each request waiting until [waitIdle] resolves true.
/// [waitReady] gates the first wave on a usable session only.
Future<void> runInboxPreload(
  ChannelPreloader preloader,
  List<InboxPreloadTarget> targets, {
  required Future<bool> Function() waitReady,
  required Future<bool> Function() waitIdle,
}) async {
  final first = targets.take(inboxPreloadRows).toList();
  final rest = targets.skip(inboxPreloadRows).toList();

  if (!await waitReady()) return;
  final firstRooms = _rooms(first);
  await preloader.preloadRooms(firstRooms);
  await preloader.preloadThreads(_threads(first), beforeRequest: waitReady);

  final loaded = {for (final room in firstRooms) room.channelId};
  final rooms = [
    for (final room in _rooms(rest))
      if (!loaded.contains(room.channelId)) room,
  ];
  for (
    var start = 0;
    start < rooms.length;
    start += maxChannelWindowsPerRequest
  ) {
    if (!await waitIdle()) return;
    await preloader.preloadRooms(
      rooms.sublist(
        start,
        math.min(start + maxChannelWindowsPerRequest, rooms.length),
      ),
    );
  }
  await preloader.preloadThreads(_threads(rest), beforeRequest: waitIdle);
}

/// Background-loads the rooms and threads behind the top chat-list [rows] so
/// opening them paints at once. The background wave pauses while the user
/// scrolls or taps, while the keyboard is up, while the app is not in the
/// foreground, and while the relay is unavailable or rate limiting. Returns a
/// callback that reports user input (for example, a pointer down).
void Function() useInboxPreload(
  WidgetRef ref,
  List<InboxItem> rows, {
  required bool enabled,
  required ScrollController scrollController,
}) {
  final context = useContext();
  final lastInputAt = useRef(DateTime.fromMillisecondsSinceEpoch(0));
  final keyboardVisible = useRef(false);
  keyboardVisible.value = MediaQuery.viewInsetsOf(context).bottom > 0;
  final markInput = useCallback(() => lastInputAt.value = DateTime.now(), []);
  useEffect(() {
    scrollController.addListener(markInput);
    return () => scrollController.removeListener(markInput);
  }, [scrollController]);

  final targets = enabled
      ? inboxPreloadTargets(rows)
      : const <InboxPreloadTarget>[];
  final targetsKey = targets.map((target) => target.key).join('|');
  useEffect(() {
    if (targets.isEmpty) return null;
    var cancelled = false;
    // Pending sleeps end at once on cancel, so no timer outlives the page.
    final sleepers = <Timer, Completer<void>>{};
    Future<void> sleep() {
      final done = Completer<void>();
      late final Timer timer;
      timer = Timer(_idlePoll, () {
        sleepers.remove(timer);
        done.complete();
      });
      sleepers[timer] = done;
      return done.future;
    }

    final preloader = ref.read(channelPreloaderProvider);
    bool isReady() =>
        preloader.canRequest &&
        (WidgetsBinding.instance.lifecycleState ?? AppLifecycleState.resumed) ==
            AppLifecycleState.resumed;
    bool isIdle() =>
        isReady() &&
        !keyboardVisible.value &&
        DateTime.now().difference(lastInputAt.value) >= _recentInput;
    Future<bool> waitUntil(bool Function() condition) async {
      while (!cancelled && !condition()) {
        await sleep();
      }
      return !cancelled;
    }

    unawaited(
      runInboxPreload(
        preloader,
        targets,
        waitReady: () => waitUntil(isReady),
        waitIdle: () => waitUntil(isIdle),
      ),
    );
    return () {
      cancelled = true;
      for (final MapEntry(key: timer, value: done) in sleepers.entries) {
        timer.cancel();
        done.complete();
      }
      sleepers.clear();
    };
  }, [targetsKey]);
  return markInput;
}
