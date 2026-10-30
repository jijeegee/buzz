import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart' show BuildContext;
import 'package:hooks_riverpod/hooks_riverpod.dart';

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
/// (docs/message-sync.md#opening-a-thread-from-the-store).
final storedThreadRepliesProvider = FutureProvider.autoDispose
    .family<List<NostrEvent>, ThreadRepliesArgs>((ref, args) async {
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

/// Replies to draw while a thread's own query is out: the inbox preload's,
/// else the device store's.
final savedThreadRepliesProvider = Provider.autoDispose
    .family<List<NostrEvent>, ThreadRepliesArgs>((ref, args) {
      final preloaded = ref.watch(preloadedThreadRepliesProvider(args));
      if (preloaded.isNotEmpty) return preloaded;
      return ref.watch(storedThreadRepliesProvider(args)).value ?? const [];
    });

/// Whether the device store's replies are still being looked up.
final savedRepliesLoadingProvider = Provider.autoDispose
    .family<bool, ThreadRepliesArgs>(
      (ref, args) => ref.watch(storedThreadRepliesProvider(args)).isLoading,
    );

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
