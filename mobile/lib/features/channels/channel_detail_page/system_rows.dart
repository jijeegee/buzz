part of '../channel_detail_page.dart';

class _SystemMessageRow extends HookConsumerWidget {
  final TimelineMessage message;
  final List<TimelineMessage>? groupedMessages;
  final String channelId;
  final String? currentPubkey;
  final List<TimelineMessage>? allMessages;
  final bool isMember;
  final bool isArchived;

  const _SystemMessageRow({
    required this.message,
    this.groupedMessages,
    required this.channelId,
    this.currentPubkey,
    this.allMessages,
    this.isMember = false,
    this.isArchived = false,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final spotlightKey = useMemoized(() => GlobalKey());
    final systemEvent = message.systemEvent;
    if (systemEvent == null) return const SizedBox.shrink();

    final userCache = ref.watch(userCacheProvider);
    final sourceMessages = groupedMessages ?? [message];
    final groupedMembership = _membershipDisplayEvent(sourceMessages);
    final messageStyleAction = switch (systemEvent.type) {
      SystemEventType.channelCreated => 'created this channel',
      SystemEventType.huddleStarted => 'started a huddle',
      SystemEventType.huddleEnded => 'ended the huddle',
      _ => null,
    };
    final messageStyleActor = messageStyleAction == null
        ? null
        : systemEvent.actorPubkey?.trim();
    final identityNames = ref.watch(channelIdentityNamesProvider(channelId));
    String resolveLabel(String? pubkey) {
      if (pubkey == null) return 'Someone';
      if (userCache[pubkey.toLowerCase()] == null) {
        ref.read(userCacheProvider.notifier).get(pubkey.toLowerCase());
      }
      return identityNames.labelFor(pubkey);
    }

    final reactions = groupedMessages == null
        ? message.reactions
        : _aggregateSystemMessageReactions(sourceMessages);

    void toggleGroupedReaction(String emoji) {
      final actions = ref.read(channelActionsProvider);
      final reactedMessages = sourceMessages.where(
        (source) => source.reactions.any(
          (reaction) =>
              reaction.emoji == emoji &&
              reaction.reactedByCurrentUser &&
              reaction.currentUserReactionId != null,
        ),
      );
      if (reactedMessages.isEmpty) {
        actions.addReaction(message.id, emoji);
        return;
      }
      for (final source in reactedMessages) {
        final reaction = source.reactions.firstWhere(
          (candidate) =>
              candidate.emoji == emoji &&
              candidate.reactedByCurrentUser &&
              candidate.currentUserReactionId != null,
        );
        actions.removeReaction(reaction.currentUserReactionId!, emoji);
      }
    }

    void openReactionPopover(Rect anchorRect) {
      final spotlightRenderObject = spotlightKey.currentContext
          ?.findRenderObject();
      final spotlightRect =
          spotlightRenderObject is RenderBox && spotlightRenderObject.hasSize
          ? spotlightRenderObject.localToGlobal(Offset.zero) &
                spotlightRenderObject.size
          : anchorRect;
      showMessageActions(
        context: context,
        ref: ref,
        message: message,
        channelId: channelId,
        canManageMessage: false,
        allMessages: null,
        currentPubkey: currentPubkey,
        isMember: isMember,
        isArchived: isArchived,
        anchorRect: spotlightRect,
        popoverSpotlightPadding: EdgeInsets.fromLTRB(
          Grid.xxs,
          Grid.xxs,
          Grid.xxs,
          reactions.isEmpty ? Grid.xxs : Grid.quarter,
        ),
      );
    }

    return Material(
      color: Colors.transparent,
      borderRadius: BorderRadius.circular(Radii.md),
      clipBehavior: Clip.antiAlias,
      child: MessageLongPressInkWell(
        key: ValueKey('system-message-row-${message.id}'),
        onLongPress: openReactionPopover,
        borderRadius: BorderRadius.circular(Radii.md),
        highlightColor: context.colors.primary.withValues(alpha: 0.1),
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: Grid.xxs),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              // Channel notices read as one centered caption so they never
              // compete with chat bubbles.
              KeyedSubtree(
                key: spotlightKey,
                child: _CenteredSystemCaption(
                  key: ValueKey('system-message-caption-${message.id}'),
                  createdAt: message.createdAt,
                  spans: groupedMembership != null
                      ? [
                          _systemActorSpan(
                            context,
                            resolveLabel(groupedMembership.targetPubkeys.first),
                          ),
                          ..._membershipActionSpans(
                            context,
                            groupedMembership,
                            resolveLabel,
                          ),
                        ]
                      : messageStyleActor != null &&
                            messageStyleActor.isNotEmpty &&
                            messageStyleAction != null
                      ? [
                          _systemActorSpan(
                            context,
                            resolveLabel(messageStyleActor),
                          ),
                          TextSpan(text: ' $messageStyleAction'),
                        ]
                      : [TextSpan(text: systemEvent.describe(resolveLabel))],
                ),
              ),
              if (systemEvent.type == SystemEventType.huddleStarted &&
                  systemEvent.ephemeralChannelId != null)
                _HuddleJoinSurface(
                  message: message,
                  allMessages: allMessages ?? const [],
                  parentChannelId: channelId,
                  isMember: isMember,
                  isArchived: isArchived,
                ),
              if (reactions.isNotEmpty)
                Center(
                  child: ReactionRow(
                    messageId: message.id,
                    channelId: channelId,
                    reactions: reactions,
                    onToggle: groupedMessages == null
                        ? (emoji) => toggleReaction(ref, message, emoji)
                        : toggleGroupedReaction,
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

const _maxVisibleAdditionalMemberNames = 3;

class _MembershipDisplayEvent {
  final String? actorPubkey;
  final List<String> targetPubkeys;
  final bool isSelfJoin;

  const _MembershipDisplayEvent({
    required this.actorPubkey,
    required this.targetPubkeys,
    required this.isSelfJoin,
  });
}

_MembershipDisplayEvent? _membershipDisplayEvent(
  List<TimelineMessage> messages,
) {
  if (messages.isEmpty) return null;

  final first = messages.first.systemEvent;
  final firstActor = first?.actorPubkey?.trim().toLowerCase();
  final firstTarget = first?.targetPubkey?.trim().toLowerCase();
  if (first?.type != SystemEventType.memberJoined ||
      firstActor == null ||
      firstActor.isEmpty ||
      firstTarget == null ||
      firstTarget.isEmpty) {
    return null;
  }

  final isSelfJoin = firstActor == firstTarget;
  final targets = <String>[];
  for (final message in messages) {
    final event = message.systemEvent;
    final actor = event?.actorPubkey?.trim().toLowerCase();
    final target = event?.targetPubkey?.trim().toLowerCase();
    if (event?.type != SystemEventType.memberJoined ||
        actor == null ||
        actor.isEmpty ||
        target == null ||
        target.isEmpty ||
        (isSelfJoin
            ? actor != target
            : actor != firstActor || actor == target)) {
      return null;
    }
    targets.add(target);
  }

  return _MembershipDisplayEvent(
    actorPubkey: isSelfJoin ? null : firstActor,
    targetPubkeys: targets,
    isSelfJoin: isSelfJoin,
  );
}

List<TimelineReaction> _aggregateSystemMessageReactions(
  List<TimelineMessage> messages,
) {
  final byEmoji = <String, List<TimelineReaction>>{};
  for (final message in messages) {
    for (final reaction in message.reactions) {
      byEmoji.putIfAbsent(reaction.emoji, () => []).add(reaction);
    }
  }

  return [
    for (final entry in byEmoji.entries)
      () {
        final reactions = entry.value;
        final userPubkeys = {
          for (final reaction in reactions) ...reaction.userPubkeys,
        }.toList();
        final reactedByCurrentUser = reactions.any(
          (reaction) => reaction.reactedByCurrentUser,
        );
        final currentUserReactionId = reactions
            .where((reaction) => reaction.currentUserReactionId != null)
            .firstOrNull
            ?.currentUserReactionId;
        final fallbackCount = reactions.fold<int>(
          0,
          (total, reaction) => total + reaction.count,
        );
        return TimelineReaction(
          emoji: entry.key,
          count: userPubkeys.isEmpty ? fallbackCount : userPubkeys.length,
          reactedByCurrentUser: reactedByCurrentUser,
          userPubkeys: userPubkeys,
          emojiUrl: reactions.first.emojiUrl,
          currentUserReactionId: currentUserReactionId,
        );
      }(),
  ];
}

List<InlineSpan> _membershipActionSpans(
  BuildContext context,
  _MembershipDisplayEvent event,
  String Function(String? pubkey) resolveLabel,
) {
  final additionalTargets = event.targetPubkeys.skip(1).toList();
  final visibleTargets = additionalTargets
      .take(_maxVisibleAdditionalMemberNames)
      .toList();
  final hiddenTargets = additionalTargets
      .skip(_maxVisibleAdditionalMemberNames)
      .toList();
  return [
    TextSpan(
      text: event.isSelfJoin
          ? ' joined the channel'
          : ' was added by ${resolveLabel(event.actorPubkey)}',
    ),
    if (additionalTargets.isNotEmpty)
      TextSpan(text: event.isSelfJoin ? ' along with ' : ', along with '),
    ..._memberNameSpans(
      context,
      visibleTargets: visibleTargets,
      hiddenTargets: hiddenTargets,
      resolveLabel: resolveLabel,
      style: _systemActionTextStyle(context),
    ),
  ];
}

InlineSpan _systemActorSpan(BuildContext context, String label) => TextSpan(
  text: label,
  style: const TextStyle(fontWeight: FontWeight.w600),
);

/// A system notice as one small, centered, muted line with its time.
class _CenteredSystemCaption extends StatelessWidget {
  final List<InlineSpan> spans;
  final int createdAt;

  const _CenteredSystemCaption({
    super.key,
    required this.spans,
    required this.createdAt,
  });

  @override
  Widget build(BuildContext context) {
    final style = _systemActionTextStyle(
      context,
    )?.copyWith(fontSize: 12, height: 16 / 12);
    return Center(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: Grid.sm),
        child: Text.rich(
          TextSpan(
            style: style,
            children: [
              ...spans,
              TextSpan(
                text: '  ${formatMessageTime(createdAt)}',
                style: TextStyle(
                  fontSize: 11,
                  color: context.colors.onSurfaceVariant.withValues(alpha: 0.7),
                ),
              ),
            ],
          ),
          textAlign: TextAlign.center,
        ),
      ),
    );
  }
}

TextStyle? _systemActionTextStyle(BuildContext context) {
  return systemMessageBodyTextStyle.copyWith(
    color: context.colors.onSurfaceVariant,
  );
}

List<InlineSpan> _memberNameSpans(
  BuildContext context, {
  required List<String> visibleTargets,
  required List<String> hiddenTargets,
  required String Function(String? pubkey) resolveLabel,
  required TextStyle? style,
}) {
  final spans = <InlineSpan>[];
  for (var index = 0; index < visibleTargets.length; index++) {
    final isLast = index == visibleTargets.length - 1;
    final separator = index == 0
        ? ''
        : isLast && hiddenTargets.isEmpty
        ? (visibleTargets.length == 2 ? ' and ' : ', and ')
        : ', ';
    spans.add(
      TextSpan(text: '$separator${resolveLabel(visibleTargets[index])}'),
    );
  }

  if (hiddenTargets.isNotEmpty) {
    final hiddenLabels = hiddenTargets.map(resolveLabel).toList();
    spans
      ..add(const TextSpan(text: ', and '))
      ..add(
        WidgetSpan(
          alignment: PlaceholderAlignment.baseline,
          baseline: TextBaseline.alphabetic,
          child: Tooltip(
            message: hiddenLabels.join('\n'),
            triggerMode: TooltipTriggerMode.tap,
            child: Text(
              '${hiddenTargets.length} others',
              key: const Key('membership-overflow'),
              style: style?.copyWith(
                decoration: TextDecoration.underline,
                decorationStyle: TextDecorationStyle.dotted,
              ),
            ),
          ),
        ),
      );
  }

  return spans;
}

class _ThreadSummaryRow extends ConsumerWidget {
  final ThreadSummary summary;
  final TimelineMessage message;
  final List<TimelineMessage> allMessages;
  final String channelId;
  final String? currentPubkey;
  final bool isMember;
  final bool isArchived;

  const _ThreadSummaryRow({
    required this.summary,
    required this.message,
    required this.allMessages,
    required this.channelId,
    required this.currentPubkey,
    required this.isMember,
    required this.isArchived,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final userCache = ref.watch(userCacheProvider);
    final name = ref
        .watch(threadNameProvider((channelId: channelId, headId: message.id)))
        .value
        ?.content;
    // The summary follows its message's bubble to the right for own messages.
    final isOwn = currentPubkey?.toLowerCase() == message.pubkey.toLowerCase();

    final summaryRow = GestureDetector(
      onTap: () {
        Navigator.of(context).push(
          MaterialPageRoute<void>(
            builder: (_) => ThreadDetailPage(
              threadHead: message,
              allMessages: allMessages,
              channelId: channelId,
              currentPubkey: currentPubkey,
              isMember: isMember,
              isArchived: isArchived,
            ),
          ),
        );
      },
      child: Padding(
        key: ValueKey('thread-summary-${message.id}'),
        padding: EdgeInsets.only(
          left: isOwn ? 0 : messageAvatarSize + messageAvatarContentGap,
          top: Grid.half,
          bottom: Grid.xs,
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            // Stacked participant avatars.
            SizedBox(
              width: 32.0 + (summary.participantPubkeys.length - 1) * 20.0,
              height: 32,
              child: Stack(
                children: [
                  for (var i = 0; i < summary.participantPubkeys.length; i++)
                    Positioned(
                      left: i * 20.0,
                      child: SmallAvatar(
                        pubkey: summary.participantPubkeys[i],
                        userCache: userCache,
                        size: 32,
                      ),
                    ),
                ],
              ),
            ),
            const SizedBox(width: Grid.xxs),
            Flexible(
              child: Text.rich(
                TextSpan(
                  children: [
                    TextSpan(
                      text:
                          '${name != null && name.isNotEmpty ? '$name · ' : ''}${summary.isCountPending ? 'Replies' : '${summary.replyCount}${summary.isLowerBound ? '+' : ''} ${summary.replyCount == 1 ? 'reply' : 'replies'}'}',
                      style: replyPreviewTextStyle.copyWith(
                        color: context.colors.primary,
                      ),
                    ),
                    if (summary.lastReplyAt case final lastReplyAt?) ...[
                      TextSpan(
                        text: ' · ',
                        style: replyPreviewTextStyle.copyWith(
                          color: context.colors.onSurfaceVariant.withValues(
                            alpha: 0.5,
                          ),
                        ),
                      ),
                      TextSpan(
                        text:
                            'last reply ${formatThreadSummaryLastReplyTime(lastReplyAt)}',
                        style: replyPreviewTextStyle.copyWith(
                          color: context.colors.onSurfaceVariant,
                        ),
                      ),
                    ],
                  ],
                ),
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
              ),
            ),
          ],
        ),
      ),
    );
    if (!isOwn) return summaryRow;
    // Keep an own message's summary within its bubble's width so a long
    // thread name doesn't stretch it back across to the left edge.
    return LayoutBuilder(
      builder: (context, constraints) => Align(
        alignment: Alignment.centerRight,
        child: ConstrainedBox(
          constraints: BoxConstraints(
            maxWidth: constraints.maxWidth * chatOwnBubbleWidthFactor,
          ),
          child: summaryRow,
        ),
      ),
    );
  }
}
