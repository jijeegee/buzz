part of '../thread_detail_page.dart';

class _ThreadMessageList extends StatelessWidget {
  final LaidOutViewport viewport;
  final VoidCallback onUserScrollStart;

  /// Receives the list's one active scroll position, so Latest can glide it
  /// directly instead of cross-fading a second list in.
  final ObjectRef<ScrollPosition?> activeScrollPosition;
  final ItemScrollController itemScrollController;
  final ItemPositionsListener itemPositionsListener;

  /// Where the list is drawn from its first frame; a new epoch draws a new
  /// list there.
  final _ThreadListPlacement placement;

  /// The reply the "New messages" divider goes above, by list index.
  final int? newRepliesIndex;
  final double bottomInset;
  final List<TimelineMessage> replies;
  final AsyncValue<List<NostrEvent>> relayReplyState;

  /// Older replies remain on the relay; they load as the head comes into view.
  final bool hasOlderReplies;
  final bool isLoadingOlderReplies;
  final VoidCallback onRetryReplies;
  final Map<String, DateTime> localSendAnimations;

  /// False while the first relay page is pending: a loading row stands in
  /// for the head so an open never paints it before the newest replies.
  final bool showsHead;
  final bool headIsDeleted;
  final TimelineMessage head;
  final ValueNotifier<int?> stickyDayTimestamp;
  final Map<String, String> channelNames;
  final String channelId;
  final String? currentPubkey;
  final String? highlightedMessageId;
  final List<TimelineMessage> allMessages;
  final bool isMember;
  final bool isArchived;
  final FocusNode composerFocusNode;
  final VoidCallback restoreComposerFocus;
  final Map<String, List<TimelineMessage>> childrenByParent;

  /// The thread composer that "Quote" fills; null when it can't send.
  final ComposerQuoteScope? quoteScope;

  const _ThreadMessageList({
    required this.viewport,
    required this.onUserScrollStart,
    required this.activeScrollPosition,
    required this.itemScrollController,
    required this.itemPositionsListener,
    required this.placement,
    this.newRepliesIndex,
    required this.bottomInset,
    required this.replies,
    required this.relayReplyState,
    this.hasOlderReplies = false,
    this.isLoadingOlderReplies = false,
    required this.onRetryReplies,
    required this.localSendAnimations,
    required this.showsHead,
    required this.headIsDeleted,
    required this.head,
    required this.stickyDayTimestamp,
    required this.channelNames,
    required this.channelId,
    required this.currentPubkey,
    required this.highlightedMessageId,
    required this.allMessages,
    required this.isMember,
    required this.isArchived,
    required this.composerFocusNode,
    required this.restoreComposerFocus,
    required this.childrenByParent,
    required this.quoteScope,
  });

  String _replySummary(AppLocalizations l10n) {
    if (hasOlderReplies) {
      return isLoadingOlderReplies
          ? l10n.threadLoadingEarlierReplies
          : l10n.threadEarlierRepliesOnScroll;
    }
    if (!relayReplyState.hasValue && replies.isEmpty) {
      return relayReplyState.isLoading
          ? l10n.threadLoadingReplies
          : l10n.threadLoadRepliesError;
    }
    final count = l10n.threadSummaryReplyCount(replies.length);
    if (!relayReplyState.hasError) return count;
    return '$count · ${relayReplyState.isLoading ? l10n.threadRepliesRetrying : l10n.threadRepliesRefreshError}';
  }

  @override
  Widget build(BuildContext context) {
    return LaidOutViewportReporter(
      viewport: viewport,
      child: KeyboardDismissOnDrag(
        key: ValueKey(placement.epoch),
        onUserScrollStart: onUserScrollStart,
        child: ScrollablePositionedList.builder(
          key: const ValueKey('thread-message-list'),
          initialScrollIndex: placement.index,
          initialAlignment: placement.alignment,
          itemScrollController: itemScrollController,
          itemPositionsListener: itemPositionsListener,
          // Bottom-anchored like the channel timeline: item 0 is the newest
          // reply against the composer, and the head closes the top end. An
          // open starts at the newest reply without first painting the head,
          // and a short thread stacks up from the composer.
          reverse: true,
          padding: EdgeInsetsDirectional.only(
            start: chatListLeftInset,
            end: chatListRightInset,
            top: frostedAppBarHeight(context),
            bottom: Grid.xs + bottomInset,
          ).resolve(Directionality.of(context)),
          itemCount: replies.length + 1,
          itemBuilder: (context, index) => _trackActiveThreadScrollPosition(
            _buildItem(context, index),
            activeScrollPosition,
          ),
        ),
      ),
    );
  }

  Widget _buildItem(BuildContext context, int index) {
    final headIndex = replies.length;
    if (index == headIndex) {
      if (!showsHead) {
        return Padding(
          key: const ValueKey('thread-replies-loading'),
          padding: const EdgeInsets.symmetric(vertical: Grid.xs),
          child: Text(
            context.l10n.threadLoadingReplies,
            style: context.textTheme.labelMedium?.copyWith(
              color: context.colors.onSurfaceVariant,
              fontWeight: FontWeight.w600,
            ),
          ),
        );
      }
      if (headIsDeleted) {
        return Padding(
          key: const ValueKey('thread-message-deleted'),
          padding: const EdgeInsets.only(bottom: Grid.xs),
          child: Text(context.l10n.threadMessageDeleted),
        );
      }
      return Padding(
        key: ValueKey('thread-message-group-${head.id}'),
        padding: const EdgeInsets.only(bottom: Grid.xs),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            DayDivider(
              label: formatDayHeading(head.createdAt),
              dayTimestamp: head.createdAt,
              stickyDayTimestamp: stickyDayTimestamp,
            ),
            BlockedMessageGate(
              authorPubkey: head.pubkey,
              channelId: channelId,
              child: _ThreadMessage(
                message: head,
                channelNames: channelNames,
                channelId: channelId,
                currentPubkey: currentPubkey,
                showAuthor: true,
                isHighlighted: head.id == highlightedMessageId,
                allMessages: allMessages,
                isMember: isMember,
                isArchived: isArchived,
                isThreadHead: true,
                composerFocusNode: composerFocusNode,
                restoreComposerFocus: restoreComposerFocus,
                quoteScope: quoteScope,
              ),
            ),
            Padding(
              padding: const EdgeInsets.symmetric(vertical: Grid.xxs),
              child: Row(
                children: [
                  Flexible(
                    child: Semantics(
                      liveRegion: true,
                      child: Text(
                        _replySummary(context.l10n),
                        style: context.textTheme.labelMedium?.copyWith(
                          color: context.colors.onSurfaceVariant,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                  ),
                  if (relayReplyState.hasError && !relayReplyState.isLoading)
                    IconButton(
                      key: const ValueKey('thread-replies-retry'),
                      onPressed: onRetryReplies,
                      tooltip: context.l10n.commonRetry,
                      icon: const Icon(LucideIcons.refreshCcw, size: 16),
                    ),
                  const SizedBox(width: Grid.xxs),
                  Expanded(
                    child: Divider(color: context.colors.outlineVariant),
                  ),
                ],
              ),
            ),
          ],
        ),
      );
    }

    // Reversed list: index 0 = newest reply.
    final chronologicalIndex = replies.length - 1 - index;
    final reply = replies[chronologicalIndex];
    final previousReply = chronologicalIndex > 0
        ? replies[chronologicalIndex - 1]
        : null;
    final previousMessage = previousReply ?? head;
    final showDayDivider = !isSameDay(
      previousMessage.createdAt,
      reply.createdAt,
    );
    final showAuthor =
        previousReply == null ||
        showDayDivider ||
        previousReply.pubkey.toLowerCase() != reply.pubkey.toLowerCase() ||
        (reply.createdAt - previousReply.createdAt) > 300;
    final nextReply = chronologicalIndex + 1 < replies.length
        ? replies[chronologicalIndex + 1]
        : null;
    final showTime =
        nextReply == null ||
        !isSameDay(reply.createdAt, nextReply.createdAt) ||
        nextReply.pubkey.toLowerCase() != reply.pubkey.toLowerCase() ||
        (nextReply.createdAt - reply.createdAt) > 300;

    // Check if this reply itself has children (nested thread).
    final nestedChildren = childrenByParent[reply.id];
    final nestedSummary = nestedChildren != null && nestedChildren.isNotEmpty
        ? _buildNestedSummary(reply.id, nestedChildren)
        : null;

    return LocalMessageSendTransition(
      key: ValueKey('thread-message-send-${reply.id}'),
      animate: isRecentLocalMessageSendAnimation(localSendAnimations, reply.id),
      startOffsetFactor: showAuthor
          ? localMessageSendTransitionAvatarStartOffset
          : localMessageSendTransitionStartOffset,
      child: Padding(
        key: ValueKey('thread-message-group-${reply.id}'),
        padding: EdgeInsets.zero,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (showDayDivider)
              DayDivider(
                label: formatDayHeading(reply.createdAt),
                dayTimestamp: reply.createdAt,
                stickyDayTimestamp: stickyDayTimestamp,
              ),
            if (index == newRepliesIndex)
              const NewMessagesDivider(
                key: ValueKey('thread-new-messages-divider'),
              ),
            BlockedMessageGate(
              authorPubkey: reply.pubkey,
              channelId: channelId,
              child: _ThreadMessage(
                message: reply,
                channelNames: channelNames,
                channelId: channelId,
                currentPubkey: currentPubkey,
                showAuthor: showAuthor,
                showTime: showTime,
                isHighlighted: reply.id == highlightedMessageId,
                allMessages: allMessages,
                isMember: isMember,
                isArchived: isArchived,
                composerFocusNode: composerFocusNode,
                restoreComposerFocus: restoreComposerFocus,
                quoteScope: quoteScope,
              ),
            ),
            if (nestedSummary != null)
              _NestedThreadSummaryRow(
                summary: nestedSummary,
                replyMessage: reply,
                allMessages: allMessages,
                channelId: channelId,
                currentPubkey: currentPubkey,
                isMember: isMember,
                isArchived: isArchived,
              ),
          ],
        ),
      ),
    );
  }
}
