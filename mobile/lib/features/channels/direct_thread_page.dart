import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/push/push_inbox_events.dart';
import '../../shared/relay/relay.dart';
import '../agents/owner_view/owner_view.dart';
import 'channel.dart';
import 'channel_detail_page.dart';
import 'channel_messages_provider.dart';
import 'channels_provider.dart';
import 'thread_detail_page.dart';
import 'thread_replies_provider.dart';
import 'thread_root_provider.dart';

typedef LinkedReplyArgs = ({String channelId, String messageId});

/// The reply a link names: the room's rows, then the store, then the relay
/// (docs/message-sync.md#opening-a-thread-from-its-root-id). Null until found;
/// read again after a push inbox drain and a reconnect until then. It does
/// not use `loadEventsById`, which waits for the socket and keeps the event
/// in the room's timeline.
final linkedReplyProvider = FutureProvider.autoDispose
    .family<NostrEvent?, LinkedReplyArgs>((ref, args) async {
      var found = false;
      void askAgain() {
        if (!found) ref.invalidateSelf();
      }

      ref.listen(pushInboxRevisionProvider, (_, _) => askAgain());
      ref.listen(relaySessionProvider, (previous, next) {
        if (previous?.status != SessionStatus.connected &&
            next.status == SessionStatus.connected) {
          askAgain();
        }
      });
      final sources = ref.read(threadRootSourcesProvider);
      final session = ref.read(relaySessionProvider.notifier);
      NostrEvent? reply;
      final room = channelMessagesProvider(args.channelId);
      if (ref.exists(room)) {
        reply =
            ref
                .read(room)
                .value
                ?.where((event) => event.id == args.messageId)
                .firstOrNull ??
            ref.read(room.notifier).deepLinkEvent(args.messageId);
      }
      if (reply == null) {
        try {
          final stored = await sources.stored(
            session,
            ThreadRepliesArgs(
              channelId: args.channelId,
              rootId: args.messageId,
            ),
          );
          reply = stored?.event;
        } catch (error) {
          debugPrint('[LinkedReply] stored reply unavailable: $error');
        }
      }
      if (reply == null) {
        try {
          final answer = await sources.relay(session, [
            NostrFilter(
              kinds: EventKind.channelTimelineContentKinds,
              ids: [args.messageId],
            ),
          ]);
          reply = answer
              .where((event) => event.id == args.messageId)
              .firstOrNull;
        } catch (error) {
          debugPrint('[LinkedReply] relay reply unavailable: $error');
        }
      }
      if (reply?.channelId != args.channelId) return null;
      found = true;
      return reply;
    });

/// The head a link's [threadId] opens, given the [linked] reply it came with.
/// A link names the reply's parent, which is the thread's root only for a
/// direct reply: a nested parent is kept with the root as [rootId], so
/// replies are read by root, and a parent the relay does not return
/// ([parentMissing]) gives way to the root.
({String headId, String? rootId}) linkedThreadHead({
  required String threadId,
  required NostrEvent? linked,
  required bool parentMissing,
}) {
  final (:parentId, :rootId) =
      linked?.threadReference ?? (parentId: null, rootId: null);
  if (parentId != threadId || rootId == null || rootId == threadId) {
    return (headId: threadId, rootId: null);
  }
  return parentMissing
      ? (headId: rootId, rootId: null)
      : (headId: threadId, rootId: rootId);
}

/// A thread as a route of its own, opened by id from anywhere: nothing here
/// waits for the head, the room's timeline or the relay
/// (docs/message-sync.md#opening-a-thread-from-its-root-id).
class DirectThreadPage extends ConsumerWidget {
  /// The room as the opener knows it; the listed room replaces it.
  final Channel channel;
  final String threadId;

  /// The reply to open at. [threadId] is then its parent, not always a root.
  final String? messageId;

  /// [channel] was built from a Chats row: the composer waits for the room.
  final bool standIn;
  final bool headMayBeNested;
  final NostrEvent? rootHint;
  final String? threadName;
  final bool archived;
  final OwnerView? ownerView;

  const DirectThreadPage({
    super.key,
    required this.channel,
    required this.threadId,
    this.messageId,
    this.standIn = false,
    this.headMayBeNested = false,
    this.rootHint,
    this.threadName,
    this.archived = false,
    this.ownerView,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final room =
        ref.watch(channelsProvider.select(selectChannel(channel.id))) ??
        channel;
    // Created before the thread query, which settles own pending replies
    // through it only when it exists.
    ref.read(channelMessagesProvider(channel.id).notifier);
    final messageId = this.messageId;
    final linked = messageId == null
        ? null
        : ref.watch(
            linkedReplyProvider((channelId: channel.id, messageId: messageId)),
          );
    final reply = linked?.value;
    final (:parentId, :rootId) =
        reply?.threadReference ?? (parentId: null, rootId: null);
    final nested = parentId == threadId && rootId != null && rootId != threadId;
    final head = linkedThreadHead(
      threadId: threadId,
      linked: reply,
      parentMissing:
          nested &&
          ref
              .watch(
                threadRootProvider(
                  ThreadRepliesArgs(channelId: channel.id, rootId: threadId),
                ),
              )
              .missing,
    );
    return ThreadDetailPage(
      key: ValueKey(head.headId),
      headId: head.headId,
      rootId: head.rootId,
      rootHint: rootHint,
      initialThreadName: threadName,
      standInChannel: standIn ? channel : null,
      channelId: channel.id,
      currentPubkey: null,
      isMember: room.isMember,
      isArchived: archived || room.isArchived,
      initialMessageId: messageId,
      initialMessageCreatedAt: reply?.createdAt,
      awaitsLinkedReply: linked != null && reply == null && linked.isLoading,
      headMayBeNested: headMayBeNested,
      // The thread is the route itself: its group button opens the group.
      channelBelow: false,
      ownerView: ownerView,
    );
  }
}

/// The page for a [threadId] in [channel], else the room, at [messageId].
/// Every way into a thread opens it through here. Forum rooms keep the room
/// page, which shows their posts.
Widget threadOrRoomPage({
  required Channel channel,
  String? threadId,
  String? messageId,
  bool standIn = false,
  bool headMayBeNested = false,
  NostrEvent? rootHint,
  String? threadName,
  bool archived = false,
  bool highlightInitialMessage = false,
  OwnerView? ownerView,
}) => threadId == null || channel.isForum
    ? ChannelDetailPage(
        channel: channel,
        initialMessageId: messageId,
        highlightInitialMessage: highlightInitialMessage,
        ownerView: ownerView,
      )
    : DirectThreadPage(
        channel: channel,
        threadId: threadId,
        messageId: messageId,
        standIn: standIn,
        headMayBeNested: headMayBeNested,
        rootHint: rootHint,
        threadName: threadName,
        archived: archived,
        ownerView: ownerView,
      );
