part of '../chats_page.dart';

/// Half of the avatar column, as desktop's By channel indent.
const double _threadIndent = (activityAvatarSize + messageAvatarContentGap) / 2;
const double _threadCircleSize = activityAvatarSize * 0.62;

/// A colour fixed per id, as the Activity conversation avatars use.
Color _hueFor(String seed) {
  var hash = 0;
  for (final unit in seed.codeUnits) {
    hash = (hash * 31 + unit) & 0x7fffffff;
  }
  return HSLColor.fromAHSL(1, (hash % 360).toDouble(), 0.55, 0.48).toColor();
}

/// "3:05 PM" today, "Yesterday", else "Oct 12" (as Activity rows).
String chatRowTimestamp(int unixSeconds, {DateTime? now}) {
  if (unixSeconds <= 0) return '';
  final current = now ?? DateTime.now();
  final date = DateTime.fromMillisecondsSinceEpoch(unixSeconds * 1000);
  final today = DateTime(current.year, current.month, current.day);
  final day = DateTime(date.year, date.month, date.day);
  final dayDiff = today.difference(day).inDays;
  if (dayDiff == 0) {
    final hour = date.hour % 12 == 0 ? 12 : date.hour % 12;
    final minute = date.minute.toString().padLeft(2, '0');
    return '$hour:$minute ${date.hour < 12 ? 'AM' : 'PM'}';
  }
  if (dayDiff == 1) return 'Yesterday';
  const months = [
    'Jan',
    'Feb',
    'Mar',
    'Apr',
    'May',
    'Jun',
    'Jul',
    'Aug',
    'Sep',
    'Oct',
    'Nov',
    'Dec',
  ];
  final label = '${months[date.month - 1]} ${date.day}';
  return date.year == current.year ? label : '$label, ${date.year}';
}

/// One Chats row, in the Activity (Inbox) row's look: avatar | name + time |
/// latest message in up to two lines, with @ and the unread count under the
/// time. No author in the preview.
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
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final unread = watchChatRowUnread(ref, row);
    final roomName = _roomName(ref, row, channel, currentPubkey);
    final title = row.isThread
        ? showChannelInTitle
              ? '$roomName › ${chatThreadTitle(row)}'
              : chatThreadTitle(row)
        : roomName;
    final latest = row.latest;
    final preview = latest == null ? '' : chatRowPreviewText(latest);
    final mutedColor = context.colors.onSurfaceVariant;
    final bold = unread.isUnread;

    final Widget avatar = row.isThread
        ? _ThreadAvatar(channelId: row.channelId, rootId: row.rootId ?? row.key)
        : row.isDm
        ? _DmAvatar(row: row, channel: channel, currentPubkey: currentPubkey)
        : _ChannelAvatar(channelId: row.channelId);

    return Opacity(
      opacity: muted ? 0.5 : 1,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('chat-row-tap-${row.key}'),
          onTap: onTap,
          child: Padding(
            padding: EdgeInsets.fromLTRB(
              Grid.gutter + (indent ? _threadIndent : 0),
              Grid.twelve,
              Grid.gutter,
              Grid.twelve,
            ),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                avatar,
                const SizedBox(width: messageAvatarContentGap),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          Flexible(
                            child: Text(
                              title,
                              key: ValueKey('chat-row-title-${row.key}'),
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: activityUsernameTextStyle.copyWith(
                                color: context.colors.onSurface,
                              ),
                            ),
                          ),
                          if (pinned) ...[
                            const SizedBox(width: Grid.quarter),
                            Icon(LucideIcons.pin, size: 12, color: mutedColor),
                          ],
                          if (muted) ...[
                            const SizedBox(width: Grid.quarter),
                            Icon(
                              LucideIcons.bellOff,
                              size: 12,
                              color: mutedColor,
                            ),
                          ],
                          const Spacer(),
                          const SizedBox(width: Grid.xxs),
                          Text(
                            chatRowTimestamp(latest?.createdAt ?? row.at),
                            style: activityTimestampTextStyle.copyWith(
                              color: mutedColor,
                            ),
                          ),
                        ],
                      ),
                      const SizedBox(height: Grid.quarter),
                      Row(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Expanded(
                            child: preview.isEmpty
                                ? const SizedBox(height: 1)
                                : MessageContent(
                                    key: ValueKey(
                                      'chat-row-preview-${row.key}',
                                    ),
                                    content: preview,
                                    tags: latest?.tags ?? const [],
                                    maxLines: 2,
                                    baseStyle: activityPreviewTextStyle
                                        .copyWith(
                                          color: bold
                                              ? context.colors.onSurface
                                              : mutedColor,
                                          fontWeight: bold
                                              ? FontWeight.w600
                                              : null,
                                        ),
                                  ),
                          ),
                          _UnreadBadges(rowKey: row.key, unread: unread),
                        ],
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A room's name: `#channel`, or the DM's people.
String _roomName(
  WidgetRef ref,
  ChatListRow row,
  Channel? channel,
  String? currentPubkey,
) {
  if (!row.isDm) {
    final name = channel?.name.trim().isNotEmpty == true
        ? channel!.name.trim()
        : row.channelName.trim();
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

class _UnreadBadges extends StatelessWidget {
  const _UnreadBadges({required this.rowKey, required this.unread});

  final String rowKey;
  final ChatRowUnread unread;

  @override
  Widget build(BuildContext context) {
    final hasMentions = unread.mentions > 0 || unread.mentionsOverflow;
    final hasCount = unread.count > 0 || unread.overflow;
    if (!hasMentions && !hasCount && !unread.marked) {
      return const SizedBox.shrink();
    }
    return Padding(
      padding: const EdgeInsets.only(left: Grid.xxs, top: Grid.quarter),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (hasMentions)
            _Pill(
              key: ValueKey('chat-row-mention-$rowKey'),
              label: '@',
              color: context.colors.primary,
              foreground: context.colors.onPrimary,
            ),
          if (hasMentions && (hasCount || unread.marked))
            const SizedBox(width: Grid.quarter),
          if (hasCount)
            _Pill(
              key: ValueKey('chat-row-unread-$rowKey'),
              label: formatChatCount(unread.count, unread.overflow),
              color: context.colors.error,
              foreground: context.colors.onError,
            )
          else if (unread.marked)
            Container(
              key: ValueKey('chat-row-marked-$rowKey'),
              width: 8,
              height: 8,
              margin: const EdgeInsets.only(top: Grid.quarter),
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: context.colors.error,
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
    constraints: const BoxConstraints(minWidth: 18),
    padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
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
        height: 1.3,
      ),
    ),
  );
}

/// A channel: its colour, with `#`.
class _ChannelAvatar extends StatelessWidget {
  const _ChannelAvatar({required this.channelId});

  final String channelId;

  @override
  Widget build(BuildContext context) => Container(
    key: const ValueKey('chat-channel-avatar'),
    width: activityAvatarSize,
    height: activityAvatarSize,
    decoration: BoxDecoration(
      color: _hueFor(channelId),
      borderRadius: BorderRadius.circular(Grid.twelve),
    ),
    alignment: Alignment.center,
    child: const Icon(LucideIcons.hash, size: 18, color: Colors.white),
  );
}

/// A thread: two circles of the same size, overlapping, centred in the
/// avatar column; the back one in the channel's colour, the front one in
/// the thread's.
class _ThreadAvatar extends StatelessWidget {
  const _ThreadAvatar({required this.channelId, required this.rootId});

  final String channelId;
  final String rootId;

  @override
  Widget build(BuildContext context) {
    const offset = activityAvatarSize - _threadCircleSize;
    final ring = Border.all(color: context.colors.surface, width: 2);
    return SizedBox.square(
      key: const ValueKey('chat-thread-avatar'),
      dimension: activityAvatarSize,
      child: Stack(
        children: [
          Positioned(
            left: 0,
            top: offset / 2,
            child: Container(
              width: _threadCircleSize,
              height: _threadCircleSize,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _hueFor(channelId).withValues(alpha: 0.55),
                border: ring,
              ),
            ),
          ),
          Positioned(
            left: offset,
            top: offset / 2,
            child: Container(
              width: _threadCircleSize,
              height: _threadCircleSize,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _hueFor(rootId),
                border: ring,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// A DM: the other person; a group DM: the first two people, overlapping.
class _DmAvatar extends HookConsumerWidget {
  const _DmAvatar({
    required this.row,
    required this.channel,
    required this.currentPubkey,
  });

  final ChatListRow row;
  final Channel? channel;
  final String? currentPubkey;

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
    const size = _threadCircleSize + 2;
    const offset = activityAvatarSize - size;
    return SizedBox.square(
      dimension: activityAvatarSize,
      child: Stack(
        children: [
          Positioned(
            left: 0,
            top: 0,
            child: _PersonAvatar(pubkey: people[0], size: size),
          ),
          Positioned(
            left: offset,
            top: offset,
            child: DecoratedBox(
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                border: Border.all(color: context.colors.surface, width: 2),
              ),
              child: _PersonAvatar(pubkey: people[1], size: size - 4),
            ),
          ),
        ],
      ),
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
    final isAgent =
        ref.watch(knownAgentPubkeysProvider).contains(pubkey) ||
        profile?.isAgent == true;
    final initial =
        profile?.initial ?? (pubkey.isNotEmpty ? pubkey[0].toUpperCase() : '?');
    return AvatarImage(
      imageUrl: profile?.avatarUrl,
      radius: size / 2,
      backgroundColor: context.colors.primaryContainer,
      fallback: Text(
        initial,
        style: TextStyle(
          fontSize: size * 0.34,
          fontWeight: FontWeight.w600,
          color: context.colors.onPrimaryContainer,
        ),
      ),
      isAgent: isAgent,
    );
  }
}

/// A channel whose threads are listed but whose room row is not.
class _ChannelHeaderRow extends StatelessWidget {
  const _ChannelHeaderRow({
    super.key,
    required this.channel,
    required this.fallbackName,
  });

  final Channel? channel;
  final String fallbackName;

  @override
  Widget build(BuildContext context) {
    final name = channel?.name.trim().isNotEmpty == true
        ? channel!.name.trim()
        : fallbackName.trim();
    return Padding(
      padding: const EdgeInsets.fromLTRB(Grid.gutter, Grid.xs, Grid.gutter, 0),
      child: Text(
        name.isEmpty ? '#channel' : '#$name',
        style: activityUsernameTextStyle.copyWith(
          color: context.colors.onSurface,
        ),
      ),
    );
  }
}

/// "+ N more threads" / "Show less" under a channel in By channel.
class _MoreThreadsRow extends StatelessWidget {
  const _MoreThreadsRow({super.key, required this.more, required this.onTap});

  final ChatGroupMore more;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final style = context.textTheme.labelMedium?.copyWith(
      color: context.colors.primary,
      fontWeight: FontWeight.w600,
    );
    final label = more.expanded
        ? 'Show less'
        : '+ ${more.hidden} more ${more.hidden == 1 ? 'thread' : 'threads'}';
    return InkWell(
      onTap: onTap,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(
          Grid.gutter +
              _threadIndent +
              activityAvatarSize +
              messageAvatarContentGap,
          Grid.xxs,
          Grid.gutter,
          Grid.twelve,
        ),
        child: Row(
          children: [
            Text(label, style: style),
            if (!more.expanded && more.hiddenUnread > 0) ...[
              const SizedBox(width: Grid.xxs),
              _Pill(
                label: formatChatCount(more.hiddenUnread, false),
                color: context.colors.error,
                foreground: context.colors.onError,
              ),
            ],
          ],
        ),
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
