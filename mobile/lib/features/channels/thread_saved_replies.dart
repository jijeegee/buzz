import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart' show BuildContext;
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/push/push_inbox_events.dart';
import '../../shared/relay/relay.dart';
import 'channel_preload.dart';
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

/// A thread's newest replies as the device store holds them, empty when it
/// holds none. Not yet confirmed: the thread's relay page replaces them
/// (docs/message-sync.md#opening-a-thread-from-the-store). Read again after
/// a push inbox drain wrote to the store.
final storedThreadRepliesProvider = FutureProvider.autoDispose
    .family<List<NostrEvent>, ThreadRepliesArgs>((ref, args) async {
      ref.watch(pushInboxRevisionProvider);
      final session = ref.read(relaySessionProvider.notifier);
      try {
        final events = await ref.read(paintableThreadHeadProvider)(
          session,
          args,
        );
        if (events == null) return const [];
        return parseThreadWindowPage(
              events,
              channelId: args.channelId,
              rootId: args.rootId,
            )?.events ??
            const [];
      } catch (error) {
        debugPrint('[ThreadReplies] stored replies unavailable: $error');
        return const [];
      }
    });

/// Replies to draw while a thread's own query is out: the inbox preload's
/// and the device store's, oldest first; the preload's copy of a reply wins.
final savedThreadRepliesProvider = Provider.autoDispose
    .family<List<NostrEvent>, ThreadRepliesArgs>((ref, args) {
      final preloaded = ref.watch(preloadedThreadRepliesProvider(args));
      final stored =
          ref.watch(storedThreadRepliesProvider(args)).value ?? const [];
      if (stored.isEmpty) return preloaded;
      if (preloaded.isEmpty) return stored;
      return mergeThreadEvents(stored, preloaded);
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
