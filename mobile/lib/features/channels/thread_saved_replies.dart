import 'dart:async';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart' show BuildContext;
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/push/push_inbox_events.dart';
import '../../shared/relay/relay.dart';
import 'channel_preload.dart';
import 'relay_cut.dart';
import 'timeline_message.dart';
import 'thread_replies_provider.dart';
import 'thread_window.dart';

/// Reads a thread's newest replies from the device store whether or not they
/// are certain, to draw before the relay answers; overridden in tests.
final paintableThreadHeadProvider = Provider<StoredThreadHeadReader>(
  (ref) =>
      (session, args) => session.paintableThreadHead(
        args.channelId,
        args.rootId,
        kinds: threadWindowKinds,
        limit: threadWindowPageReplies,
      ),
);

/// A device store read of a thread: its replies, and when those only the
/// push inbox wrote were fetched.
typedef StoredThreadReplies = ({
  List<NostrEvent> events,
  Map<String, int?> pushedAt,
});

const StoredThreadReplies _noStoredReplies = (events: [], pushedAt: {});

/// A thread's newest replies as the device store holds them, empty when it
/// holds none. Not yet confirmed: the thread's relay page replaces them
/// (docs/message-sync.md#opening-a-thread-from-the-store). Read again after
/// a push inbox drain wrote to the store.
final storedThreadRepliesProvider = FutureProvider.autoDispose
    .family<StoredThreadReplies, ThreadRepliesArgs>((ref, args) async {
      ref.watch(pushInboxRevisionProvider);
      final session = ref.read(relaySessionProvider.notifier);
      try {
        final events = await ref.read(paintableThreadHeadProvider)(
          session,
          args,
        );
        if (events == null) return _noStoredReplies;
        final page = parseThreadWindowPage(
          events,
          channelId: args.channelId,
          rootId: args.rootId,
        );
        if (page == null) return _noStoredReplies;
        return (
          events: page.events,
          pushedAt: storedPushedAt(events, threadWindowBoundsKind),
        );
      } catch (error) {
        debugPrint('[ThreadReplies] stored replies unavailable: $error');
        return _noStoredReplies;
      }
    });

/// Stored replies past the relay cut of a thread already holding a window,
/// the same rule as a room's (docs/message-sync.md#reading-the-store-again).
/// While newer pages remain they would sit past replies not loaded.
final storedNewerRepliesProvider = Provider.autoDispose
    .family<List<NostrEvent>, ThreadRepliesArgs>((ref, args) {
      final window = ref.watch(threadWindowProvider(args));
      final stored = ref.watch(storedThreadRepliesProvider(args)).value;
      if (window == null || stored == null || window.state.hasNewer) {
        return const [];
      }
      final cut = ref.read(threadWindowProvider(args).notifier).relayCut;
      final held = {for (final reply in window.events) reply.id};
      final covered = window.state.newestCovered?.createdAt ?? 0;
      return [
        for (final reply in stored.events)
          if (!held.contains(reply.id) &&
              isPastRelayCut(
                cut,
                createdAt: reply.createdAt,
                fetchedAt: stored.pushedAt[reply.id],
                newestHeld: covered,
              ))
            reply,
      ];
    });

/// Replies to draw while a thread's own query is out: the inbox preload's,
/// with the device store's newer than its newest (a push drained since),
/// oldest first; else the device store's. Older stored replies are left
/// out: the preload, from the relay, may have dropped them as deleted.
final savedThreadRepliesProvider = Provider.autoDispose
    .family<List<NostrEvent>, ThreadRepliesArgs>((ref, args) {
      final preloaded = ref.watch(preloadedThreadRepliesProvider(args));
      final stored =
          ref.watch(storedThreadRepliesProvider(args)).value?.events ??
          const [];
      if (preloaded.isEmpty) return stored;
      final newest = preloaded.map((reply) => reply.createdAt).reduce(max);
      final newer = [
        for (final reply in stored)
          if (reply.createdAt > newest) reply,
      ];
      return newer.isEmpty ? preloaded : mergeThreadEvents(preloaded, newer);
    });

/// Whether the device store's replies are still being looked up for the
/// first time; a read again after a drain keeps the replies it had.
final savedRepliesLoadingProvider = Provider.autoDispose
    .family<bool, ThreadRepliesArgs>((ref, args) {
      final stored = ref.watch(storedThreadRepliesProvider(args));
      return stored.isLoading && !stored.hasValue;
    });

/// The held reply a thread opens at on the first frame: [id] among
/// [replies], and for a first [unread] one an older reply too.
TimelineMessage? heldReplyToOpenAt(
  List<TimelineMessage> replies,
  String id, {
  required bool unread,
}) {
  final held = replies.where((reply) => reply.id == id).firstOrNull;
  if (held == null) return null;
  return !unread || replies.any((r) => r.createdAt < held.createdAt)
      ? held
      : null;
}

/// Opens [window] around [held] (a saved reply the thread opened at), with
/// [pending] set until its pages land or fail: the relay's newest page may
/// not hold that reply.
void openAroundSavedReply(
  ThreadWindowNotifier window,
  ValueNotifier<bool> pending,
  TimelineMessage held,
  BuildContext context,
) {
  pending.value = true;
  unawaited(
    window
        .openAt(createdAt: held.createdAt, eventId: held.id)
        .catchError((Object _) => false)
        .whenComplete(() {
          if (context.mounted) pending.value = false;
        }),
  );
}
