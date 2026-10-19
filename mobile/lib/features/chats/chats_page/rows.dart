part of '../chats_page.dart';

/// Half of the avatar column, as desktop's By channel indent.
const double _threadIndent = (activityAvatarSize + messageAvatarContentGap) / 2;

/// The `@` badge colour (desktop `sky-600`).
const Color _mentionBadgeColor = Color(0xFF0284C7);

/// "3:05 PM" today, "Yesterday", else "Oct 12" (as Activity rows).
String chatRowTimestamp(int unixSeconds, {DateTime? now}) =>
    unixSeconds <= 0 ? '' : conversationTimestamp(unixSeconds, now: now);

/// One Chats row: the Activity conversation row ([ConversationRowLayout])
/// with desktop's Chats row rules: avatar | title, then the latest message in
/// up to two lines | a right column with the time on top and @ and the unread
/// count under it.
/// The preview never runs under the right column. No author in the preview.
class _ChatRow extends HookConsumerWidget {
  const _ChatRow({
    super.key,
    required this.row,
    required this.channel,
    required this.currentPubkey,
    required this.showChannelInTitle,
    required this.indent,
    required this.pinned,
    required this.muted,
    required this.threadUnread,
    this.working,
    required this.onTap,
  });

  final ChatListRow row;
  final Channel? channel;
  final String? currentPubkey;

  /// All chats: a thread reads `#channel › thread`.
  final bool showChannelInTitle;

  /// By channel: a thread sits half an avatar in, under its channel.
  final bool indent;
  final bool pinned;
  final bool muted;

  /// A room row whose channel has an unread listed thread (a dot when the
  /// room itself has nothing to count).
  final bool threadUnread;

  /// Agents working in this chat now (left of the time).
  final ChatWorkingSummary? working;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final rowUnread = watchChatRowUnread(ref, row);
    // Desktop: a muted room counts as unread only with a mention for me;
    // otherwise it shows no count and dims.
    final unread =
        !row.isThread &&
            muted &&
            rowUnread.mentions == 0 &&
            !rowUnread.mentionsOverflow
        ? ChatRowUnread.none
        : rowUnread;
    final roomName = _roomName(ref, row, channel, currentPubkey);
    final threadTitle = row.isThread ? chatThreadTitle(row) : null;
    final latest = row.latest;
    final preview = latest == null ? '' : _watchPreview(ref, latest);
    final mutedColor = context.colors.onSurfaceVariant;
    final background = conversationListBackground(context);
    final hasUnread = unread.isUnread;
    // Desktop: a room dims when muted with nothing unread; a thread when
    // closed, or muted without a mention for me.
    final dimmed = row.isThread
        ? row.closed || (muted && unread.mentions == 0)
        : muted && !hasUnread;
    final bold = row.isThread ? hasUnread && !dimmed : hasUnread;

    final Widget avatar = row.isThread
        ? ConversationThreadAvatar(
            key: const ValueKey('chat-thread-avatar'),
            channelSeed: row.channelId,
            channelLabel: _bareRoomName(row, channel),
            threadSeed: row.rootId ?? row.key,
            threadLabel: threadTitle!,
            ringColor: background,
          )
        : row.isDm
        ? _DmAvatar(
            row: row,
            channel: channel,
            currentPubkey: currentPubkey,
            ringColor: background,
          )
        : ConversationLetterAvatar(
            seed: row.channelId,
            label: _bareRoomName(row, channel),
            avatarKey: const ValueKey('chat-channel-avatar'),
          );

    final titleStyle = activityUsernameTextStyle.copyWith(
      color: context.colors.onSurface,
    );
    final Widget title = row.isThread && showChannelInTitle
        ? LayoutBuilder(
            builder: (context, constraints) => Row(
              children: [
                ConstrainedBox(
                  constraints: BoxConstraints(
                    maxWidth: constraints.maxWidth * 0.45,
                  ),
                  child: Text(
                    roomName,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: titleStyle.copyWith(
                      color: mutedColor,
                      fontWeight: FontWeight.w400,
                    ),
                  ),
                ),
                Text(
                  ' › ',
                  style: titleStyle.copyWith(
                    color: mutedColor.withValues(alpha: 0.6),
                    fontWeight: FontWeight.w400,
                  ),
                ),
                Flexible(
                  child: Text(
                    threadTitle!,
                    key: ValueKey('chat-row-title-${row.key}'),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: titleStyle,
                  ),
                ),
              ],
            ),
          )
        : Text(
            threadTitle ?? roomName,
            key: ValueKey('chat-row-title-${row.key}'),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: titleStyle,
          );

    final titleLineHeight = conversationTitleLineHeight(context);
    final content = ConversationRowLayout(
      startInset: indent ? _threadIndent : 0,
      avatar: avatar,
      title: SizedBox(
        height: titleLineHeight,
        child: Row(
          children: [
            Flexible(child: title),
            if (row.isThread && row.closed) ...[
              const SizedBox(width: Grid.quarter),
              Icon(
                LucideIcons.circleCheck,
                key: ValueKey('chat-row-closed-${row.key}'),
                size: 13,
                color: mutedColor,
              ),
            ],
            if (pinned) ...[
              const SizedBox(width: Grid.quarter),
              Icon(
                LucideIcons.pin,
                key: ValueKey('chat-row-pinned-${row.key}'),
                size: 12,
                color: mutedColor,
              ),
            ],
            if (muted && !row.isThread) ...[
              const SizedBox(width: Grid.quarter),
              Icon(
                LucideIcons.bellOff,
                key: ValueKey('chat-row-muted-${row.key}'),
                size: 12,
                color: mutedColor,
              ),
            ],
          ],
        ),
      ),
      preview: preview.isEmpty
          ? null
          : Text(
              preview,
              key: ValueKey('chat-row-preview-${row.key}'),
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: activityPreviewTextStyle.copyWith(
                color: bold ? context.colors.onSurface : mutedColor,
                fontWeight: bold ? FontWeight.w600 : null,
              ),
            ),
      side: Column(
        key: ValueKey('chat-row-meta-${row.key}'),
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          SizedBox(
            height: titleLineHeight,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (working case final working?) ...[
                  _WorkingBadge(rowKey: row.key, summary: working),
                  const SizedBox(width: Grid.half),
                ],
                Text(
                  chatRowTimestamp(latest?.createdAt ?? row.at),
                  key: ValueKey('chat-row-time-${row.key}'),
                  style: activityTimestampTextStyle.copyWith(color: mutedColor),
                ),
              ],
            ),
          ),
          _RowBadges(
            rowKey: row.key,
            unread: unread,
            muted: muted,
            threadUnread: threadUnread && !row.isDm && !row.isThread,
          ),
        ],
      ),
    );

    return Opacity(
      key: ValueKey('chat-row-opacity-${row.key}'),
      opacity: dimmed ? 0.5 : 1,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('chat-row-tap-${row.key}'),
          onTap: onTap,
          child: content,
        ),
      ),
    );
  }

  /// The cleaned preview with each `@mention` by its cached name.
  String _watchPreview(WidgetRef ref, NostrEvent latest) {
    final author = latest.pubkey.toLowerCase();
    final blocked = ref.watch(
      blockedPubkeysProvider.select(
        (ids) => ids.any((id) => id.toLowerCase() == author),
      ),
    );
    final text = chatRowPreviewText(latest, isBlocked: (_) => blocked);
    if (blocked) return text;
    final tagged = mentionedPubkeysFromTags(latest.tags);
    if (tagged.isEmpty) return text;
    final directory = ref.watch(agentDirectoryDisplayNamesProvider);
    final names = <String, String>{};
    for (final pubkey in tagged) {
      final name = ref.watch(
        userCacheProvider.select((cache) => cache[pubkey]?.displayName),
      );
      final resolved = name?.trim().isNotEmpty == true
          ? name!
          : directory[pubkey];
      if (resolved != null) names[pubkey] = resolved;
    }
    return resolvePreviewMentions(text, latest.tags, names);
  }
}

/// The room's name without `#`: the channel's, or the DM's people.
String _bareRoomName(ChatListRow row, Channel? channel) {
  final name = channel?.name.trim().isNotEmpty == true
      ? channel!.name.trim()
      : row.channelName.trim();
  return name;
}

/// A room's name: `#channel`, or the DM's people.
String _roomName(
  WidgetRef ref,
  ChatListRow row,
  Channel? channel,
  String? currentPubkey,
) {
  if (!row.isDm) {
    final name = _bareRoomName(row, channel);
    return name.isEmpty ? '#channel' : '#$name';
  }
  if (channel != null) {
    return resolveDmChannelDisplayLabel(
      channel,
      currentPubkey: currentPubkey,
      names: ref.watch(identityNameSourcesProvider),
    );
  }
  final me = currentPubkey?.toLowerCase();
  final others = [
    for (final pubkey in row.dmMembers)
      if (pubkey.toLowerCase() != me) pubkey.toLowerCase(),
  ];
  final labels = [
    for (final pubkey in others)
      ref.watch(userCacheProvider.select((cache) => cache[pubkey]?.label)) ??
          shortPubkey(pubkey),
  ];
  return labels.isEmpty ? row.channelName : labels.join(', ');
}

/// Under the time: `@` when a mention waits, then the unread count (grey when
/// muted), a dot for a row marked unread, or a dot for unread threads.
class _RowBadges extends StatelessWidget {
  const _RowBadges({
    required this.rowKey,
    required this.unread,
    required this.muted,
    required this.threadUnread,
  });

  final String rowKey;
  final ChatRowUnread unread;
  final bool muted;
  final bool threadUnread;

  @override
  Widget build(BuildContext context) {
    final hasMentions = unread.mentions > 0 || unread.mentionsOverflow;
    final hasCount = unread.count > 0 || unread.overflow;
    final dot = !hasCount && (unread.marked || threadUnread);
    if (!hasMentions && !hasCount && !dot) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(top: conversationTitlePreviewGap),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (hasMentions)
            _Pill(
              key: ValueKey('chat-row-mention-$rowKey'),
              label: '@',
              color: _mentionBadgeColor,
              foreground: Colors.white,
            ),
          if (hasMentions && (hasCount || dot))
            const SizedBox(width: Grid.half),
          if (hasCount)
            _Pill(
              key: ValueKey('chat-row-unread-$rowKey'),
              label: formatChatCount(unread.count, unread.overflow),
              color: muted
                  ? context.colors.onSurface.withValues(alpha: 0.2)
                  : context.colors.primary,
              foreground: muted
                  ? context.colors.onSurface
                  : context.colors.onPrimary,
            )
          else if (dot)
            Container(
              key: ValueKey(
                unread.marked
                    ? 'chat-row-marked-$rowKey'
                    : 'chat-row-thread-unread-$rowKey',
              ),
              width: 8,
              height: 8,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: context.colors.primary,
              ),
            ),
        ],
      ),
    );
  }
}

/// "● 3m 12s" in a light pill while agents work, ticking each second;
/// "(N)" when several agents work (desktop's channel working badge).
class _WorkingBadge extends HookWidget {
  const _WorkingBadge({required this.rowKey, required this.summary});

  final String rowKey;
  final ChatWorkingSummary summary;

  @override
  Widget build(BuildContext context) {
    final now = useState(DateTime.now().millisecondsSinceEpoch);
    useEffect(() {
      final timer = Timer.periodic(
        const Duration(seconds: 1),
        (_) => now.value = DateTime.now().millisecondsSinceEpoch,
      );
      return timer.cancel;
    }, const []);
    final primary = context.colors.primary;
    return Container(
      key: ValueKey('chat-row-working-$rowKey'),
      height: 16,
      padding: const EdgeInsets.symmetric(horizontal: 6),
      decoration: BoxDecoration(
        color: primary.withValues(alpha: 0.1),
        borderRadius: BorderRadius.circular(Radii.full),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 5,
            height: 5,
            decoration: BoxDecoration(shape: BoxShape.circle, color: primary),
          ),
          const SizedBox(width: 3),
          Text(
            chatWorkingLabel(summary, now.value),
            style: TextStyle(
              fontSize: 11,
              height: 1,
              fontWeight: FontWeight.w500,
              color: primary,
              fontFeatures: const [FontFeature.tabularFigures()],
            ),
          ),
        ],
      ),
    );
  }
}

class _Pill extends StatelessWidget {
  const _Pill({
    super.key,
    required this.label,
    required this.color,
    required this.foreground,
  });

  final String label;
  final Color color;
  final Color foreground;

  @override
  Widget build(BuildContext context) => Container(
    constraints: const BoxConstraints(minWidth: 20, minHeight: 20),
    padding: const EdgeInsets.symmetric(horizontal: 5),
    alignment: Alignment.center,
    decoration: BoxDecoration(
      color: color,
      borderRadius: BorderRadius.circular(Radii.full),
    ),
    child: Text(
      label,
      textAlign: TextAlign.center,
      style: context.textTheme.labelSmall?.copyWith(
        color: foreground,
        fontWeight: FontWeight.w700,
        height: 1.2,
      ),
    ),
  );
}

/// A DM: the other person; a group DM: the first two people side by side,
/// overlapping, as thread avatars.
class _DmAvatar extends HookConsumerWidget {
  const _DmAvatar({
    required this.row,
    required this.channel,
    required this.currentPubkey,
    required this.ringColor,
  });

  final ChatListRow row;
  final Channel? channel;
  final String? currentPubkey;
  final Color ringColor;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final me = currentPubkey?.toLowerCase();
    final members = channel?.participantPubkeys ?? row.dmMembers;
    final others = [
      for (final pubkey in members)
        if (pubkey.toLowerCase() != me) pubkey.toLowerCase(),
    ];
    final people = others.isEmpty && me != null ? [me] : others;
    useEffect(() {
      final missing = [
        for (final pubkey in people.take(2))
          if (ref.read(userCacheProvider)[pubkey] == null) pubkey,
      ];
      if (missing.isNotEmpty) {
        Future.microtask(
          () => ref.read(userCacheProvider.notifier).preload(missing),
        );
      }
      return null;
    }, [people.join(',')]);
    if (people.length < 2) {
      return _PersonAvatar(
        pubkey: people.firstOrNull ?? '',
        size: activityAvatarSize,
      );
    }
    return ConversationAvatarPair(
      key: const ValueKey('chat-group-dm-avatar'),
      ringColor: ringColor,
      back: _PersonAvatar(pubkey: people[0], size: conversationPairCircleSize),
      front: _PersonAvatar(pubkey: people[1], size: conversationPairCircleSize),
    );
  }
}

class _PersonAvatar extends ConsumerWidget {
  const _PersonAvatar({required this.pubkey, required this.size});

  final String pubkey;
  final double size;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final profile = ref.watch(
      userCacheProvider.select((cache) => cache[pubkey]),
    );
    return ConversationPersonAvatar(
      pubkey: pubkey,
      profile: profile,
      isAgent:
          ref.watch(knownAgentPubkeysProvider).contains(pubkey) ||
          profile?.isAgent == true,
      size: size,
      fontSize: size / 3,
    );
  }
}

/// A channel whose threads are listed but whose room row is not.
class _ChannelHeaderRow extends StatelessWidget {
  const _ChannelHeaderRow({
    super.key,
    required this.channel,
    required this.fallbackName,
    this.working,
  });

  final Channel? channel;
  final String fallbackName;

  /// Agents working in the channel's room now.
  final ChatWorkingSummary? working;

  @override
  Widget build(BuildContext context) {
    final name = channel?.name.trim().isNotEmpty == true
        ? channel!.name.trim()
        : fallbackName.trim();
    return Padding(
      padding: const EdgeInsets.fromLTRB(Grid.gutter, Grid.xs, Grid.gutter, 0),
      child: Row(
        children: [
          Expanded(
            child: Text(
              name.isEmpty ? '#channel' : '#$name',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: activityUsernameTextStyle.copyWith(
                color: context.colors.onSurface,
              ),
            ),
          ),
          if (working case final working?) ...[
            const SizedBox(width: Grid.xxs),
            _WorkingBadge(
              rowKey: 'channel-${channel?.id ?? fallbackName}',
              summary: working,
            ),
          ],
        ],
      ),
    );
  }
}

/// "+ N more threads" under a channel in By channel. Each tap shows 10 more:
/// "+ 10 more · N left" (or "+ N more" for the last ones) beside "Show less";
/// with every thread shown, only "Show less".
class _MoreThreadsRow extends StatelessWidget {
  const _MoreThreadsRow({
    super.key,
    required this.more,
    required this.onShowMore,
    required this.onShowLess,
  });

  final ChatGroupMore more;
  final VoidCallback onShowMore;
  final VoidCallback onShowLess;

  @override
  Widget build(BuildContext context) {
    final style = context.textTheme.labelMedium?.copyWith(
      color: context.colors.primary,
      fontWeight: FontWeight.w600,
    );
    final hidden = more.hidden;
    final label = !more.expanded
        ? '+ $hidden more ${hidden == 1 ? 'thread' : 'threads'}'
        : hidden > byChannelThreadStep
        ? '+ $byChannelThreadStep more · $hidden left'
        : '+ $hidden more';
    final showMore = Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(label, style: style),
        if (more.hiddenUnread > 0) ...[
          const SizedBox(width: Grid.xxs),
          _Pill(
            label: formatChatCount(more.hiddenUnread, false),
            color: context.colors.primary,
            foreground: context.colors.onPrimary,
          ),
        ],
      ],
    );
    const padding = EdgeInsets.fromLTRB(
      Grid.gutter +
          _threadIndent +
          activityAvatarSize +
          messageAvatarContentGap,
      0,
      Grid.gutter,
      Grid.twelve,
    );
    final showLess = Text('Show less', style: style);
    // One control: the whole line taps, as before.
    if (hidden == 0 || !more.expanded) {
      return InkWell(
        key: ValueKey(
          hidden == 0
              ? 'chat-more-less-${more.channelId}'
              : 'chat-more-show-${more.channelId}',
        ),
        onTap: hidden == 0 ? onShowLess : onShowMore,
        child: Padding(
          padding: padding,
          child: hidden == 0 ? Row(children: [showLess]) : showMore,
        ),
      );
    }
    return Padding(
      padding: padding,
      child: Row(
        children: [
          InkWell(
            key: ValueKey('chat-more-show-${more.channelId}'),
            onTap: onShowMore,
            child: showMore,
          ),
          const Spacer(),
          InkWell(
            key: ValueKey('chat-more-less-${more.channelId}'),
            onTap: onShowLess,
            child: showLess,
          ),
        ],
      ),
    );
  }
}

class _ViewModeSwitch extends StatelessWidget {
  const _ViewModeSwitch({required this.mode, required this.onChanged});

  final ChatsViewMode mode;
  final ValueChanged<ChatsViewMode> onChanged;

  @override
  Widget build(BuildContext context) {
    Widget option(ChatsViewMode value, String label) {
      final selected = mode == value;
      return Expanded(
        child: GestureDetector(
          key: ValueKey('chats-view-${value.name}'),
          behavior: HitTestBehavior.opaque,
          onTap: () {
            if (!selected) {
              unawaited(HapticFeedback.selectionClick());
              onChanged(value);
            }
          },
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 160),
            margin: const EdgeInsets.all(3),
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: selected ? context.colors.surface : Colors.transparent,
              borderRadius: BorderRadius.circular(Grid.xxs),
              boxShadow: selected
                  ? [
                      BoxShadow(
                        color: context.colors.shadow.withValues(alpha: 0.08),
                        blurRadius: 3,
                        offset: const Offset(0, 1),
                      ),
                    ]
                  : null,
            ),
            child: Text(
              label,
              style: context.textTheme.labelLarge?.copyWith(
                color: selected
                    ? context.colors.onSurface
                    : context.colors.onSurfaceVariant,
                fontWeight: selected ? FontWeight.w700 : FontWeight.w500,
              ),
            ),
          ),
        ),
      );
    }

    return Semantics(
      container: true,
      label: 'Chats view',
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: context.colors.surfaceContainerHighest.withValues(alpha: 0.6),
          borderRadius: BorderRadius.circular(Grid.xs),
        ),
        child: Row(
          children: [
            option(ChatsViewMode.all, 'All chats'),
            option(ChatsViewMode.byChannel, 'By channel'),
          ],
        ),
      ),
    );
  }
}

class _ChatsEmptyState extends StatelessWidget {
  const _ChatsEmptyState({
    required this.onFindChannels,
    required this.onMessageSomeone,
  });

  final VoidCallback onFindChannels;
  final VoidCallback onMessageSomeone;

  @override
  Widget build(BuildContext context) => Center(
    child: Padding(
      padding: const EdgeInsets.all(Grid.lg),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            LucideIcons.messagesSquare,
            size: 40,
            color: context.colors.onSurfaceVariant,
          ),
          const SizedBox(height: Grid.xs),
          Text('No chats yet', style: context.textTheme.titleMedium),
          const SizedBox(height: Grid.sm),
          Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              OutlinedButton(
                key: const ValueKey('chats-empty-find-channels'),
                onPressed: onFindChannels,
                child: const Text('Find channels'),
              ),
              const SizedBox(width: Grid.xs),
              FilledButton(
                key: const ValueKey('chats-empty-message'),
                onPressed: onMessageSomeone,
                child: const Text('Message someone'),
              ),
            ],
          ),
        ],
      ),
    ),
  );
}
