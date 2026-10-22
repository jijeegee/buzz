part of '../activity_page.dart';

const _inboxSwipeActionInset = Grid.half;
const _inboxSwipeActionLabelMinWidth = Grid.xxl;
const _inboxSwipeLabelRevealWidth =
    _inboxSwipeActionLabelMinWidth + (_inboxSwipeActionInset * 2);

/// "New" boundary between unread and previously read rows.
class _NewBoundaryDivider extends StatelessWidget {
  const _NewBoundaryDivider();

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Grid.gutter,
        vertical: Grid.half,
      ),
      child: Row(
        children: [
          Expanded(
            child: Divider(color: context.colors.error.withValues(alpha: 0.5)),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: Grid.xxs),
            child: Text(
              'New',
              style: context.textTheme.labelSmall?.copyWith(
                color: context.colors.error,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          Expanded(
            child: Divider(color: context.colors.error.withValues(alpha: 0.5)),
          ),
        ],
      ),
    );
  }
}

/// One conversation row, matching desktop's inbox item hierarchy:
/// avatar | sender + unread dot + time | contextual label | preview.
///
/// Swiping left reveals the row's read-state action. Opening remains a tap or
/// long-press action, so the swipe has one clear, easily recoverable outcome.
class _InboxRow extends HookConsumerWidget {
  final InboxItem item;
  final Channel? channel;
  final String? currentPubkey;
  final bool isDone;

  /// The relay's unread count for this row, shown when above one.
  final InboxServerUnread? serverUnread;
  final VoidCallback onTap;
  final VoidCallback onMarkRead;
  final VoidCallback onMarkUnread;

  /// Chat-list presentation for the Channels + Threads view: the room's
  /// avatar and name lead the row and the sender is omitted.
  final bool conversationView;

  const _InboxRow({
    super.key,
    required this.item,
    required this.channel,
    required this.currentPubkey,
    required this.isDone,
    this.serverUnread,
    required this.onTap,
    required this.onMarkRead,
    required this.onMarkUnread,
    this.conversationView = false,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    const restingRevealWidth = 132.0;
    const commitThreshold = 0.58;
    final revealAmount = useState(0.0);
    final isDragging = useState(false);
    final labelHapticFired = useRef(false);
    final senderPubkey = item.item.pubkey.toLowerCase();
    final mentionPubkeys = mentionedPubkeysFromTags(item.item.tags);
    final relevantPubkeys = {senderPubkey, ...mentionPubkeys};
    final profiles = <String, UserProfile?>{
      for (final pubkey in relevantPubkeys)
        pubkey: ref.watch(userCacheProvider.select((cache) => cache[pubkey])),
    };
    final profile = profiles[senderPubkey];
    // The shared label contract: blank cached names (empty or whitespace-only
    // are relay-valid) fall back to the compact npub, never a blank sender.
    // Rows compare names within their own channel, like the channel itself.
    final channelId = channel?.id ?? item.item.channelId;
    final Map<String, String> contextualLabels;
    if (channelId == null) {
      // No channel: the row's own identities are the comparison context.
      final names = watchIdentityNames(ref, relevantPubkeys);
      contextualLabels = {
        for (final key in names.candidates) key: names.labelFor(key),
      };
    } else {
      contextualLabels = watchChannelIdentityLabels(
        ref,
        channelId,
        relevantPubkeys,
      );
    }
    final senderLabel =
        contextualLabels[senderPubkey] ??
        profile?.label ??
        shortPubkey(item.item.pubkey);
    final profileMentionNames = {
      for (final pubkey in mentionPubkeys)
        if (profiles[pubkey]?.displayName?.trim().isNotEmpty == true)
          pubkey: profiles[pubkey]!.displayName!.trim(),
    };
    final knownAgentPubkeys = channel == null
        ? ref.watch(knownAgentPubkeysProvider)
        : ref.watch(agentMentionPubkeysProvider(channel!.id));
    final isAgent =
        knownAgentPubkeys.contains(senderPubkey) ||
        profile?.ownerPubkey != null;
    final agentMentionPubkeys = agentPubkeysWithProfileOwners(
      knownAgentPubkeys: knownAgentPubkeys,
      profileOwnedAgentPubkeys: [
        for (final pubkey in mentionPubkeys)
          if (profiles[pubkey]?.ownerPubkey != null) pubkey,
      ],
    );
    final mentionNames = mentionNamesWithDirectoryLabels(
      mentionPubkeys: mentionPubkeys,
      profileMentionNames: profileMentionNames,
      directoryDisplayNames: ref.watch(agentDirectoryDisplayNamesProvider),
      agentMentionPubkeys: agentMentionPubkeys,
    );

    final isDm = channel?.isDm ?? false;
    final feedChannelName = item.item.channelName.trim();
    // A chat of people reads by its resolved title.
    final chatTitle = channel?.isGroupChat == true
        ? watchChannelTitle(ref, channel!)
        : null;
    final channelName = isDm
        ? null
        : chatTitle ??
              (channel != null && channel!.name.isNotEmpty
                  ? channel!.name
                  : feedChannelName.isNotEmpty
                  ? feedChannelName
                  : null);
    final threadRootId = isDm ? null : item.threadRootId;
    final threadName = channelId != null && threadRootId != null
        ? ref
              .watch(
                threadNameProvider((
                  channelId: channelId,
                  headId: threadRootId,
                )),
              )
              .value
              ?.content
        : null;
    // In the conversations view the row is the chat room, not the message.
    final room = conversationView
        ? resolveConversationRoom(
            item,
            channel: channel,
            currentPubkey: currentPubkey,
            senderLabel: senderLabel,
            names: ref.watch(identityNameSourcesProvider),
            chatTitle: chatTitle,
          )
        : null;
    final roomTitle = room == null
        ? null
        : conversationRoomTitle(room, threadName: threadName);
    final channelChipLabel = channelName == null
        ? null
        : threadRootId == null
        ? channelName
        : '$channelName › ${threadName ?? 'Thread'}';
    final label = inboxTypeLabel(
      item,
      channelName: channelName,
      isDm: isDm,
      senderLabel: senderLabel,
    );

    final mutedColor = context.colors.onSurfaceVariant;
    // Like desktop: one unread message is just the dot.
    final unread = serverUnread;
    final unreadCountLabel = unread == null
        ? null
        : unread.overflow
        ? '$inboxSummaryUnreadCap+ unread'
        : unread.count > 1
        ? '${unread.count} unread'
        : null;
    final labelColor = item.isActionRequired && !isDone
        ? context.colors.tertiary
        : mutedColor;

    final reducedMotion = MediaQuery.of(context).disableAnimations;
    final swipeDirection = Directionality.of(context) == TextDirection.ltr
        ? 1.0
        : -1.0;
    void closeActions() => revealAmount.value = 0;
    void toggleReadState() {
      closeActions();
      isDone ? onMarkUnread() : onMarkRead();
    }

    return LayoutBuilder(
      builder: (context, constraints) {
        final actionExtent = constraints.maxWidth;
        final revealedWidth = revealAmount.value
            .clamp(0, actionExtent)
            .toDouble();
        final actionColor = isDone
            ? context.colors.primary
            : context.appColors.success;
        return ClipRect(
          child: Stack(
            children: [
              if (revealedWidth > 0)
                PositionedDirectional(
                  top: 0,
                  end: 0,
                  bottom: 0,
                  width: revealedWidth,
                  child: Padding(
                    key: ValueKey('inbox-swipe-background-${item.id}'),
                    padding: const EdgeInsets.all(_inboxSwipeActionInset),
                    child: _InboxSwipeAction(
                      key: ValueKey('inbox-swipe-read-${item.id}'),
                      color: actionColor,
                      foregroundColor: contrastForeground(actionColor),
                      icon: isDone ? LucideIcons.mail : LucideIcons.mailOpen,
                      label: isDone ? 'Mark unread' : 'Mark as read',
                      onTap: toggleReadState,
                    ),
                  ),
                ),
              AnimatedSlide(
                offset: Offset(
                  -swipeDirection * revealAmount.value / constraints.maxWidth,
                  0,
                ),
                duration: reducedMotion || isDragging.value
                    ? Duration.zero
                    : const Duration(milliseconds: 160),
                curve: Curves.easeOutCubic,
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onHorizontalDragStart: (_) {
                    isDragging.value = true;
                    labelHapticFired.value = false;
                  },
                  onHorizontalDragUpdate: (details) {
                    isDragging.value = true;
                    final previous = revealAmount.value;
                    final next =
                        (previous - (details.delta.dx * swipeDirection))
                            .clamp(0, actionExtent)
                            .toDouble();
                    if (!labelHapticFired.value &&
                        previous < _inboxSwipeLabelRevealWidth &&
                        next >= _inboxSwipeLabelRevealWidth) {
                      labelHapticFired.value = true;
                      unawaited(HapticFeedback.selectionClick());
                    }
                    revealAmount.value = next;
                  },
                  onHorizontalDragEnd: (details) {
                    isDragging.value = false;
                    final velocity = details.primaryVelocity ?? 0;
                    final shouldCommit =
                        (velocity * swipeDirection) < -900 ||
                        revealAmount.value >= actionExtent * commitThreshold;
                    if (shouldCommit) {
                      toggleReadState();
                    } else {
                      revealAmount.value =
                          (velocity * swipeDirection) < -200 ||
                              revealAmount.value >= restingRevealWidth / 2
                          ? restingRevealWidth
                          : 0;
                    }
                  },
                  child: Material(
                    color: context.colors.surface,
                    child: InkWell(
                      key: ValueKey('inbox-row-${item.id}'),
                      onTap: onTap,
                      onLongPress: () => _showRowActions(context),
                      child: ConversationRowLayout(
                        avatar: room != null
                            ? _ConversationRoomAvatar(
                                room: room,
                                knownAgentPubkeys: knownAgentPubkeys,
                              )
                            : ConversationPersonAvatar(
                                pubkey: item.item.pubkey,
                                profile: profile,
                                isAgent: isAgent,
                              ),
                        // Room name + time + unread dot, or sender + dot +
                        // timestamp over the contextual label.
                        title: roomTitle != null
                            ? Text(
                                roomTitle,
                                key: ValueKey(
                                  'activity-conversation-${item.id}',
                                ),
                                style: activityUsernameTextStyle.copyWith(
                                  color: context.colors.onSurface,
                                ),
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                              )
                            : Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  Row(
                                    children: [
                                      Expanded(
                                        child: MessageAuthorMeta(
                                          displayName: senderLabel,
                                          username: messageUsernameLabel(
                                            profile,
                                          ),
                                          timestamp: _inboxTimestamp(
                                            item.latestActivityAt,
                                          ),
                                          nameColor: context.colors.onSurface,
                                          metadataColor: mutedColor,
                                          nameStyle: activityUsernameTextStyle,
                                          timestampStyle:
                                              activityTimestampTextStyle,
                                          displayNameKey: ValueKey(
                                            'activity-author-${item.id}',
                                          ),
                                          usernameKey: ValueKey(
                                            'activity-username-${item.id}',
                                          ),
                                          timestampKey: ValueKey(
                                            'activity-timestamp-${item.id}',
                                          ),
                                        ),
                                      ),
                                      if (!isDone) ...[
                                        const SizedBox(width: Grid.xxs),
                                        Container(
                                          key: ValueKey(
                                            'inbox-unread-dot-${item.id}',
                                          ),
                                          width: 6,
                                          height: 6,
                                          decoration: BoxDecoration(
                                            shape: BoxShape.circle,
                                            color: context.colors.primary,
                                          ),
                                        ),
                                      ],
                                    ],
                                  ),
                                  const SizedBox(height: Grid.quarter),
                                  // Contextual label: "Mentioned in group" etc.
                                  Row(
                                    children: [
                                      Flexible(
                                        child: Text(
                                          label.text,
                                          style: activityContextTextStyle
                                              .copyWith(color: labelColor),
                                          overflow: TextOverflow.ellipsis,
                                        ),
                                      ),
                                      if (label.channelLabel != null) ...[
                                        const SizedBox(width: Grid.half),
                                        Flexible(
                                          child: Container(
                                            padding: const EdgeInsets.symmetric(
                                              horizontal:
                                                  Grid.half + Grid.quarter,
                                              vertical: Grid.quarter / 2,
                                            ),
                                            decoration: BoxDecoration(
                                              color: context
                                                  .colors
                                                  .surfaceContainerHighest,
                                              borderRadius:
                                                  BorderRadius.circular(
                                                    Grid.half,
                                                  ),
                                            ),
                                            child: Text(
                                              channelChipLabel ??
                                                  label.channelLabel!,
                                              style: activityContextTextStyle
                                                  .copyWith(color: mutedColor),
                                              overflow: TextOverflow.ellipsis,
                                            ),
                                          ),
                                        ),
                                      ],
                                    ],
                                  ),
                                ],
                              ),
                        titleTrailing: [
                          if (roomTitle != null) ...[
                            if (unreadCountLabel != null) ...[
                              Text(
                                unreadCountLabel,
                                key: ValueKey('inbox-unread-count-${item.id}'),
                                style: activityTimestampTextStyle.copyWith(
                                  color: mutedColor,
                                ),
                              ),
                              const SizedBox(width: Grid.xxs),
                            ],
                            Text(
                              _inboxTimestamp(item.latestActivityAt),
                              style: activityTimestampTextStyle.copyWith(
                                color: mutedColor,
                              ),
                            ),
                            if (!isDone) ...[
                              const SizedBox(width: Grid.xxs),
                              Container(
                                key: ValueKey('inbox-unread-dot-${item.id}'),
                                width: 6,
                                height: 6,
                                decoration: BoxDecoration(
                                  shape: BoxShape.circle,
                                  color: context.colors.primary,
                                ),
                              ),
                            ],
                          ],
                        ],
                        // Message preview.
                        preview: MessageContent(
                          content: item.item.displayContent,
                          mentionNames: mentionNames,
                          mentionLabels: contextualLabels,
                          agentMentionPubkeys: agentMentionPubkeys,
                          tags: item.item.tags,
                          maxLines: 2,
                          baseStyle: activityPreviewTextStyle.copyWith(
                            color: context.colors.onSurface,
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }

  void _showRowActions(BuildContext context) {
    showKuzzModalBottomSheet<void>(
      context: context,
      builder: (sheetContext) => SafeArea(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(
            Grid.gutter,
            0,
            Grid.gutter,
            Grid.xs,
          ),
          child: IconTheme.merge(
            data: const IconThemeData(size: 18),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                ListTile(
                  leading: Icon(
                    isDone ? LucideIcons.mail : LucideIcons.mailOpen,
                  ),
                  title: Text(isDone ? 'Mark unread' : 'Mark as read'),
                  onTap: () {
                    Navigator.of(sheetContext).pop();
                    isDone ? onMarkUnread() : onMarkRead();
                  },
                ),
                ListTile(
                  leading: const Icon(LucideIcons.externalLink),
                  title: const Text('Open conversation'),
                  onTap: () {
                    Navigator.of(sheetContext).pop();
                    onTap();
                  },
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _InboxSwipeAction extends StatelessWidget {
  final Color color;
  final Color foregroundColor;
  final IconData icon;
  final String label;
  final VoidCallback onTap;

  const _InboxSwipeAction({
    super.key,
    required this.color,
    required this.foregroundColor,
    required this.icon,
    required this.label,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) => Material(
    color: color,
    borderRadius: BorderRadius.circular(Radii.full),
    clipBehavior: Clip.antiAlias,
    child: InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(Radii.full),
      child: Semantics(
        button: true,
        label: label,
        child: LayoutBuilder(
          builder: (context, constraints) {
            if (constraints.maxWidth < Grid.lg) {
              return const SizedBox.shrink();
            }
            final showLabel =
                constraints.maxWidth >= _inboxSwipeActionLabelMinWidth;
            return Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Icon(icon, size: 18, color: foregroundColor),
                if (showLabel) ...[
                  const SizedBox(height: Grid.quarter),
                  Padding(
                    padding: const EdgeInsets.symmetric(horizontal: Grid.xxs),
                    child: Text(
                      label,
                      maxLines: 2,
                      softWrap: true,
                      textAlign: TextAlign.center,
                      style: context.textTheme.labelSmall?.copyWith(
                        color: foregroundColor,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                ],
              ],
            );
          },
        ),
      ),
    ),
  );
}

/// Room avatar for the Channels + Threads view. A DM room's picture is the
/// other participant's profile; channel and thread rooms share the channel's
/// letter on a color fixed per channel, and a thread adds a badge.
class _ConversationRoomAvatar extends HookConsumerWidget {
  final ConversationRoom room;
  final Set<String> knownAgentPubkeys;

  const _ConversationRoomAvatar({
    required this.room,
    required this.knownAgentPubkeys,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final room = this.room;
    final personPubkey = switch (room) {
      DmConversationRoom(:final personPubkey) => personPubkey,
      _ => null,
    };
    final personProfile = ref.watch(
      userCacheProvider.select(
        (cache) => personPubkey == null ? null : cache[personPubkey],
      ),
    );
    final roomChannelId = switch (room) {
      ChannelConversationRoom(:final channelId) => channelId,
      ThreadConversationRoom(:final channelId) => channelId,
      _ => null,
    };
    final roomChannel = ref.watch(
      channelsProvider.select((channels) {
        if (roomChannelId == null || roomChannelId.isEmpty) return null;
        for (final channel in channels.value ?? const <Channel>[]) {
          if (channel.id == roomChannelId) return channel;
        }
        return null;
      }),
    );
    final groupPicture = roomChannel?.picture;
    useEffect(() {
      if (personPubkey != null && personProfile == null) {
        Future.microtask(
          () => ref.read(userCacheProvider.notifier).preload([personPubkey]),
        );
      }
      return null;
    }, [personPubkey]);

    switch (room) {
      case DmConversationRoom(:final personPubkey):
        return ConversationPersonAvatar(
          pubkey: personPubkey,
          profile: personProfile,
          isAgent:
              knownAgentPubkeys.contains(personPubkey) ||
              personProfile?.ownerPubkey != null ||
              personProfile?.isAgent == true,
        );
      case ChannelConversationRoom() when roomChannel?.isGroupChat == true:
        return ChatGridAvatar(memberPubkeys: roomChannel!.participantPubkeys);
      case ChannelConversationRoom(:final channelId, :final channelName):
        return ConversationLetterAvatar(
          seed: channelId.isEmpty ? channelName : channelId,
          label: channelName,
          isThread: false,
          imageUrl: groupPicture,
        );
      case ThreadConversationRoom(:final channelId, :final channelName):
        return ConversationLetterAvatar(
          seed: channelId.isEmpty ? channelName : channelId,
          label: channelName,
          isThread: true,
          imageUrl: groupPicture,
        );
    }
  }
}
