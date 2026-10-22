part of '../search_page.dart';

/// The other people in a DM, lower-cased.
Set<String> _dmPartners(Channel channel, String? me) => {
  for (final pubkey in channel.participantPubkeys)
    if (pubkey.toLowerCase() != me) pubkey.toLowerCase(),
};

/// Opens a chat found by name as its Chats row does: a thread opens straight
/// into the thread.
void _openChatResult(BuildContext context, SearchChatResult result) {
  final channel = result.channel;
  if (channel == null) {
    ScaffoldMessenger.maybeOf(
      context,
    )?.showSnackBar(SnackBar(content: Text(context.l10n.searchStillLoading)));
    return;
  }
  final rootId = result.thread?.rootId;
  Navigator.of(context).push(
    MaterialPageRoute<void>(
      builder: (_) => ChannelDetailPage(
        channel: channel,
        initialThreadRootId: rootId,
        initialThreadRouteBehavior: rootId == null
            ? InitialThreadRouteBehavior.push
            : InitialThreadRouteBehavior.replaceCurrentRoute,
      ),
    ),
  );
}

/// Chats found by name (groups or DMs, with their threads), best match
/// first.
class _ChatsSection extends StatelessWidget {
  final String label;
  final String id;
  final List<SearchChatResult> results;
  final String? currentPubkey;
  final VoidCallback onResultSelected;

  const _ChatsSection({
    required this.label,
    required this.id,
    required this.results,
    required this.currentPubkey,
    required this.onResultSelected,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _SectionLabel(label: label, id: id),
        for (final result in results)
          if (result.isThread)
            _ThreadResultTile(
              result: result,
              onResultSelected: onResultSelected,
            )
          else if (result.channel == null || result.channel!.isPersonChat)
            _DmResultTile(
              result: result,
              currentPubkey: currentPubkey,
              onResultSelected: onResultSelected,
            )
          else
            _ChannelResultTile(
              channel: result.channel!,
              onResultSelected: onResultSelected,
            ),
      ],
    );
  }
}

class _ChannelResultTile extends StatelessWidget {
  final Channel channel;
  final VoidCallback onResultSelected;

  const _ChannelResultTile({
    required this.channel,
    required this.onResultSelected,
  });

  @override
  Widget build(BuildContext context) {
    return ListTile(
      key: ValueKey('search-channel-row-${channel.id}'),
      contentPadding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
      leading: GroupAvatar(
        key: ValueKey('search-channel-leading-${channel.id}'),
        channel: channel,
        size: 28,
      ),
      title: Text(
        channel.name,
        key: ValueKey('search-channel-title-${channel.id}'),
        style: contentListTitleTextStyle,
      ),
      subtitle: channel.isMember
          ? Text(
              context.l10n.searchMemberCount(channel.memberCount),
              style: contentListBodyTextStyle.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            )
          : null,
      trailing: !channel.isMember && !channel.isDm
          ? Container(
              padding: const EdgeInsets.symmetric(
                horizontal: Grid.half + 2,
                vertical: 3,
              ),
              decoration: BoxDecoration(
                color: context.colors.primary.withValues(alpha: 0.1),
                borderRadius: BorderRadius.circular(Radii.sm),
              ),
              child: Text(
                context.l10n.searchPublic,
                style: context.textTheme.labelSmall?.copyWith(
                  color: context.colors.primary,
                  fontWeight: FontWeight.w600,
                ),
              ),
            )
          : null,
      onTap: () {
        onResultSelected();
        Navigator.of(context).push(
          MaterialPageRoute<void>(
            builder: (_) => ChannelDetailPage(channel: channel),
          ),
        );
      },
    );
  }
}

/// The Chats thread picture at [size]: the group's circle (or photo) behind,
/// the thread's in front.
class _ThreadAvatar extends ConsumerWidget {
  final String channelId;
  final String channelName;
  final String threadSeed;
  final String threadTitle;
  final double size;

  const _ThreadAvatar({
    super.key,
    required this.channelId,
    required this.channelName,
    required this.threadSeed,
    required this.threadTitle,
    required this.size,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final picture = ref.watch(
      channelsProvider.select((channels) {
        for (final channel in channels.value ?? const <Channel>[]) {
          if (channel.id == channelId) return channel.picture;
        }
        return null;
      }),
    );
    return SizedBox.square(
      dimension: size,
      child: FittedBox(
        child: ConversationThreadAvatar(
          channelSeed: channelId,
          channelLabel: channelName,
          threadSeed: threadSeed,
          threadLabel: threadTitle,
          ringColor: context.colors.surface,
          channelPicture: picture,
        ),
      ),
    );
  }
}

/// A thread found by its title: the Chats thread avatar, the title, and the
/// group (or DM) it is in.
class _ThreadResultTile extends StatelessWidget {
  final SearchChatResult result;
  final VoidCallback onResultSelected;

  const _ThreadResultTile({
    required this.result,
    required this.onResultSelected,
  });

  @override
  Widget build(BuildContext context) {
    final thread = result.thread!;
    return ListTile(
      key: ValueKey('search-thread-row-${result.key}'),
      contentPadding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
      leading: _ThreadAvatar(
        key: ValueKey('search-thread-leading-${result.key}'),
        channelId: result.channelId,
        channelName: result.channelName,
        threadSeed: thread.rootId ?? thread.key,
        threadTitle: result.title,
        size: 28,
      ),
      title: Text(
        result.title,
        key: ValueKey('search-thread-title-${result.key}'),
        style: contentListTitleTextStyle,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      subtitle: Text(
        context.l10n.searchThreadIn(result.channelName),
        key: ValueKey('search-thread-context-${result.key}'),
        style: contentListBodyTextStyle.copyWith(
          color: context.colors.onSurfaceVariant,
        ),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      onTap: () {
        onResultSelected();
        _openChatResult(context, result);
      },
    );
  }
}

/// An existing DM found by its people's names.
class _DmResultTile extends ConsumerWidget {
  final SearchChatResult result;
  final String? currentPubkey;
  final VoidCallback onResultSelected;

  const _DmResultTile({
    required this.result,
    required this.currentPubkey,
    required this.onResultSelected,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final channel = result.channel;
    final partners = channel == null
        ? const <String>{}
        : _dmPartners(channel, currentPubkey?.toLowerCase());
    return ListTile(
      key: ValueKey('search-dm-row-${result.channelId}'),
      contentPadding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
      leading: SizedBox.square(
        dimension: 28,
        child: channel != null && channel.isGroupChat
            ? ChatGridAvatar(
                memberPubkeys: channel.participantPubkeys,
                size: 28,
              )
            : partners.length == 1
            ? SmallAvatar(
                pubkey: partners.single,
                userCache: ref.watch(userCacheProvider),
                size: 28,
              )
            : Icon(
                LucideIcons.messagesSquare,
                size: 28,
                color: context.colors.onSurfaceVariant,
              ),
      ),
      title: Text(
        result.title,
        key: ValueKey('search-dm-title-${result.channelId}'),
        style: contentListTitleTextStyle,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      onTap: () {
        onResultSelected();
        _openChatResult(context, result);
      },
    );
  }
}

class _PeopleSection extends ConsumerWidget {
  final List<DirectoryUser> users;
  final VoidCallback onResultSelected;

  const _PeopleSection({required this.users, required this.onResultSelected});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    // The shown results are the comparison context.
    final names = watchIdentityNames(
      ref,
      [for (final user in users) user.pubkey],
      agentPubkeys: {
        for (final user in users)
          if (user.isAgent) user.pubkey,
      },
      fallbackNames: {for (final user in users) user.pubkey: user.label},
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _SectionLabel(label: context.l10n.searchSectionPeople),
        for (final user in users)
          ListTile(
            key: ValueKey('search-person-row-${user.pubkey}'),
            contentPadding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
            leading: AvatarImage(
              key: ValueKey('search-person-leading-${user.pubkey}'),
              imageUrl: user.avatarUrl,
              radius: 20,
              fallback: Text(user.initial),
              isAgent: user.isAgent,
            ),
            title: Text(
              names.labelFor(user.pubkey),
              key: ValueKey('search-person-title-${user.pubkey}'),
              style: contentListTitleTextStyle,
            ),
            subtitle: Text(
              user.secondaryLabel,
              style: contentListBodyTextStyle.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
            onTap: () async {
              onResultSelected();
              final channel = await ref
                  .read(channelActionsProvider)
                  .openDm(pubkeys: [user.pubkey]);
              if (!context.mounted) return;
              await Navigator.of(context).push(
                MaterialPageRoute<void>(
                  builder: (_) => ChannelDetailPage(channel: channel),
                ),
              );
            },
          ),
      ],
    );
  }
}

class _MessagesSection extends HookConsumerWidget {
  final List<SearchHit> hits;
  final String? currentPubkey;
  final VoidCallback onResultSelected;

  const _MessagesSection({
    required this.hits,
    required this.currentPubkey,
    required this.onResultSelected,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final profiles = ref.watch(userCacheProvider);
    final channels = ref.watch(channelsProvider).value ?? [];
    final chatRows = ref.watch(chatsRowsProvider) ?? const <ChatListRow>[];

    // Preload author profiles.
    final preloadPubkeys = {
      for (final hit in hits) hit.pubkey.toLowerCase(),
      for (final hit in hits) ...mentionedPubkeysFromTags(hit.tags),
    }.toList()..sort();
    final preloadPubkeysKey = preloadPubkeys.join('\u0000');
    useEffect(() {
      ref.read(userCacheProvider.notifier).preload(preloadPubkeys);
      return null;
    }, [preloadPubkeysKey]);

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _SectionLabel(label: context.l10n.searchSectionMessages),
        for (final hit in hits)
          _MessageTile(
            hit: hit,
            thread: searchHitThread(hit),
            chatRows: chatRows,
            authorProfile: profiles[hit.pubkey.toLowerCase()],
            userCache: profiles,
            channel: channels.where((c) => c.id == hit.channelId).firstOrNull,
            currentPubkey: currentPubkey,
            onResultSelected: onResultSelected,
          ),
      ],
    );
  }
}

class _MessageTile extends ConsumerWidget {
  final SearchHit hit;

  /// The thread the hit was posted in; null on a main timeline.
  final ({String rootId, String? parentId})? thread;
  final List<ChatListRow> chatRows;
  final UserProfile? authorProfile;
  final Map<String, UserProfile> userCache;
  final Channel? channel;
  final String? currentPubkey;
  final VoidCallback onResultSelected;

  const _MessageTile({
    required this.hit,
    required this.thread,
    required this.chatRows,
    required this.authorProfile,
    required this.userCache,
    required this.channel,
    required this.currentPubkey,
    required this.onResultSelected,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final mentionedPubkeys = mentionedPubkeysFromTags(hit.tags);
    // A hit is labelled within its channel, as it would be there.
    final hitChannelId = channel?.id ?? hit.channelId;
    final labelPubkeys = {hit.pubkey.toLowerCase(), ...mentionedPubkeys};
    final Map<String, String> labels;
    if (hitChannelId == null) {
      final names = watchIdentityNames(ref, labelPubkeys);
      labels = {
        for (final pubkey in labelPubkeys) pubkey: names.labelFor(pubkey),
      };
    } else {
      labels = watchChannelIdentityLabels(ref, hitChannelId, labelPubkeys);
    }
    final authorName =
        labels[hit.pubkey.toLowerCase()] ??
        authorProfile?.label ??
        shortPubkey(hit.pubkey);
    final timeAgo = relativeTime(hit.createdAt);
    final channelName = hit.channelName?.trim().replaceFirst(RegExp(r'^#'), '');
    final hasChannelName = channelName != null && channelName.isNotEmpty;
    final isDm = channel?.isDm ?? false;
    final thread = this.thread;
    // A chat of people is named by its title, not the hit's stored name.
    final chatTitle = channel?.isGroupChat == true
        ? watchChannelTitle(ref, channel!)
        : null;
    final threadTitle = thread == null
        ? null
        : searchHitThreadTitle(thread, chatRows);
    // The chat on top, as its Chats row reads: "group › thread" in a
    // thread, the group, the chat's title, or the DM's name.
    final dmName = isDm && channel != null
        ? resolveDmChannelDisplayLabel(
            channel!,
            currentPubkey: currentPubkey,
            names: ref.watch(identityNameSourcesProvider),
          )
        : null;
    final roomName = chatTitle ?? dmName;
    final place = threadTitle == null
        ? (roomName ?? (hasChannelName ? channelName : null))
        : [
            if (roomName != null) roomName else if (hasChannelName) channelName,
            threadTitle,
          ].join(' › ');
    final profileMentionNames = {
      for (final pubkey in mentionedPubkeysFromTags(hit.tags))
        if (userCache[pubkey]?.displayName?.trim().isNotEmpty == true)
          pubkey: userCache[pubkey]!.displayName!.trim(),
    };
    final mentionPubkeys = mentionedPubkeysFromTags(hit.tags);
    final knownAgentPubkeys = channel == null
        ? ref.watch(knownAgentPubkeysProvider)
        : ref.watch(agentMentionPubkeysProvider(channel!.id));
    final agentMentionPubkeys = agentPubkeysWithProfileOwners(
      knownAgentPubkeys: knownAgentPubkeys,
      profileOwnedAgentPubkeys: [
        for (final profile in userCache.values)
          if (profile.ownerPubkey != null) profile.pubkey,
      ],
    );
    final mentionNames = mentionNamesWithDirectoryLabels(
      mentionPubkeys: mentionPubkeys,
      profileMentionNames: profileMentionNames,
      directoryDisplayNames: ref.watch(agentDirectoryDisplayNamesProvider),
      agentMentionPubkeys: agentMentionPubkeys,
    );

    return ListTile(
      key: ValueKey('search-message-row-${hit.eventId}'),
      contentPadding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
      titleAlignment: ListTileTitleAlignment.top,
      horizontalTitleGap: messageAvatarContentGap,
      leading: thread != null
          ? _ThreadAvatar(
              key: ValueKey('search-message-thread-avatar-${hit.eventId}'),
              channelId: hitChannelId ?? '',
              channelName: hasChannelName ? channelName : (channel?.name ?? ''),
              threadSeed: thread.rootId,
              threadTitle: threadTitle!,
              size: compactMessageAvatarSize,
            )
          : channel != null && !isDm
          ? GroupAvatar(
              key: ValueKey('search-message-avatar-${hit.eventId}'),
              channel: channel!,
              size: compactMessageAvatarSize,
            )
          : SmallAvatar(
              key: ValueKey('search-message-avatar-${hit.eventId}'),
              pubkey: hit.pubkey,
              userCache: userCache,
              size: compactMessageAvatarSize,
            ),
      title: MessageAuthorMeta(
        key: ValueKey('search-message-context-${hit.eventId}'),
        displayName:
            place ??
            (isDm
                ? context.l10n.searchDirectMessage
                : context.l10n.searchMessage),
        timestamp: timeAgo,
        nameColor: context.colors.onSurface,
        metadataColor: context.colors.onSurfaceVariant,
        displayNameKey: ValueKey('search-message-channel-${hit.eventId}'),
        timestampKey: ValueKey('search-message-timestamp-${hit.eventId}'),
      ),
      // The author goes with the text: "name: message".
      subtitle: Padding(
        padding: const EdgeInsets.only(top: 2),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 140),
              child: Text(
                context.l10n.searchAuthorPrefix(authorName),
                key: ValueKey('search-message-author-${hit.eventId}'),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: activityPreviewTextStyle.copyWith(
                  color: context.colors.onSurfaceVariant,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            Expanded(
              child: MessageContent(
                key: ValueKey('search-message-body-${hit.eventId}'),
                content: hit.content,
                mentionNames: mentionNames,
                mentionLabels: labels,
                agentMentionPubkeys: agentMentionPubkeys,
                tags: hit.tags,
                maxLines: 2,
                baseStyle: activityPreviewTextStyle.copyWith(
                  color: context.colors.onSurface,
                ),
              ),
            ),
          ],
        ),
      ),
      onTap: () {
        onResultSelected();
        _navigateToHit(context, hit, channel);
      },
    );
  }

  /// Opens the message itself: a forum post or comment in its post, a
  /// thread reply in its thread, any other message on its timeline,
  /// scrolled to and highlighted.
  void _navigateToHit(BuildContext context, SearchHit hit, Channel? channel) {
    if (channel == null) return;
    final forumPostId = hit.kind == 45001
        ? hit.eventId
        : hit.kind == 45003
        ? threadReferenceOf(hit.tags).rootId
        : null;
    final thread = this.thread;
    final Widget page;
    if (forumPostId != null) {
      page = ForumThreadPage(
        channelId: channel.id,
        postEventId: forumPostId,
        currentPubkey: currentPubkey,
        isMember: channel.isMember,
        isArchived: channel.isArchived,
      );
    } else if (thread != null) {
      page = ChannelDetailPage(
        channel: channel,
        initialMessageId: hit.eventId,
        initialThreadRootId: thread.parentId ?? thread.rootId,
        initialThreadRouteBehavior:
            InitialThreadRouteBehavior.replaceCurrentRoute,
      );
    } else {
      page = ChannelDetailPage(
        channel: channel,
        initialMessageId: hit.eventId,
        highlightInitialMessage: true,
      );
    }
    Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => page));
  }
}

class _SectionLabel extends StatelessWidget {
  final String label;

  /// Key suffix; defaults to the lower-cased [label].
  final String? id;

  const _SectionLabel({required this.label, this.id});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(
        Grid.gutter,
        Grid.xs,
        Grid.gutter,
        Grid.half,
      ),
      child: Text(
        label,
        key: ValueKey('search-section-${id ?? label.toLowerCase()}'),
        style: activityContextTextStyle.copyWith(
          color: context.colors.onSurfaceVariant,
        ),
      ),
    );
  }
}
