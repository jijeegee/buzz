part of '../channel_detail_page.dart';

class _MessageBubble extends HookConsumerWidget {
  final TimelineMessage message;
  final bool showAuthor;
  final bool hasReplies;
  final Map<String, String> channelNames;
  final String currentChannelId;
  final String? currentPubkey;
  final List<TimelineMessage>? allMessages;
  final bool isMember;
  final bool isArchived;
  final FocusNode? composerFocusNode;
  final VoidCallback? restoreComposerFocus;
  final ComposerQuoteScope? quoteScope;

  const _MessageBubble({
    required this.message,
    required this.showAuthor,
    required this.hasReplies,
    required this.channelNames,
    required this.currentChannelId,
    required this.currentPubkey,
    this.allMessages,
    this.isMember = false,
    this.isArchived = false,
    this.composerFocusNode,
    this.restoreComposerFocus,
    this.quoteScope,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final messageSnapshotKey = useMemoized(GlobalKey.new, const []);
    final hasLocalReplies = ref.watch(
      threadLocalRepliesProvider(
        ThreadRepliesArgs(
          channelId: currentChannelId,
          rootId: message.rootId ?? message.id,
        ),
      ).select(
        (replies) => replies.any((reply) {
          final thread = reply.threadReference;
          return thread.parentId == message.id || thread.rootId == message.id;
        }),
      ),
    );
    // Watch only this user's profile to avoid rebuilding on unrelated cache changes.
    final pk = message.pubkey.toLowerCase();
    final profile =
        ref.watch(userCacheProvider.select((cache) => cache[pk])) ??
        ref.read(userCacheProvider.notifier).get(pk);
    final displayName = watchChannelIdentityLabel(ref, currentChannelId, pk);
    final isAgent =
        ref.watch(agentMentionPubkeysProvider(currentChannelId)).contains(pk) ||
        profile?.ownerPubkey != null;
    // Only the signed-in account's own messages sit on the right. Unlike
    // [canManageMessage], this excludes agents the user owns.
    final isOwnMessage = currentPubkey?.toLowerCase() == pk;
    final canManageMessage =
        isOwnMessage ||
        (profile?.ownerPubkey != null &&
            profile?.ownerPubkey == currentPubkey?.toLowerCase());

    // Watch only profiles referenced by this message. A batched profile fetch
    // should not rebuild every visible message just because an unrelated user
    // was added to the shared cache.
    final normalizedMentionPubkeys = {
      for (final pubkey in message.mentionPubkeys) pubkey.toLowerCase(),
    };
    final mentionProfiles = <String, UserProfile?>{
      for (final pubkey in normalizedMentionPubkeys)
        pubkey: ref.watch(userCacheProvider.select((cache) => cache[pubkey])),
    };
    final knownAgentPubkeys = agentPubkeysWithProfileOwners(
      knownAgentPubkeys: ref.watch(
        agentMentionPubkeysProvider(currentChannelId),
      ),
      profileOwnedAgentPubkeys: [
        for (final entry in mentionProfiles.entries)
          if (entry.value?.ownerPubkey != null) entry.key,
      ],
    );
    final mentionNames = <String, String>{};
    final agentMentionPubkeys = <String>{};
    for (final mpk in message.mentionPubkeys) {
      final normalizedPubkey = mpk.toLowerCase();
      final p = mentionProfiles[normalizedPubkey];
      if (p?.displayName != null) {
        mentionNames[normalizedPubkey] = p!.displayName!;
      }
      if (knownAgentPubkeys.contains(normalizedPubkey)) {
        agentMentionPubkeys.add(normalizedPubkey);
      }
    }
    final resolvedMentionNames = mentionNamesWithDirectoryLabels(
      mentionPubkeys: message.mentionPubkeys,
      profileMentionNames: mentionNames,
      directoryDisplayNames: ref.watch(agentDirectoryDisplayNamesProvider),
      agentMentionPubkeys: agentMentionPubkeys,
    );
    final mentionLabels = watchChannelIdentityLabels(
      ref,
      currentChannelId,
      normalizedMentionPubkeys,
    );

    final quoteTarget = quoteScope == null
        ? null
        : quoteTargetFor(message, author: displayName);
    void openMessageActions(MessageLongPressDetails details) {
      showMessageActions(
        context: context,
        ref: ref,
        message: message,
        channelId: currentChannelId,
        canManageMessage: canManageMessage,
        allMessages: allMessages,
        currentPubkey: currentPubkey,
        isMember: isMember,
        isArchived: isArchived,
        anchorRect: details.anchorRect,
        captureAnchorSnapshot: details.captureSnapshot,
        onPopoverPreviewVisibilityChanged: details.setSourceHidden,
        onPopoverDismissed: () => details.setSourceHidden(false),
        composerFocusNode: composerFocusNode,
        restoreComposerFocus: restoreComposerFocus,
        onQuote: quoteTarget == null
            ? null
            : () {
                ref
                    .read(composerQuoteProvider(quoteScope!).notifier)
                    .quote(quoteTarget);
                restoreComposerFocus?.call();
              },
      );
    }

    return Padding(
      padding: EdgeInsets.only(top: showAuthor ? Grid.xxs : 0),
      child: Material(
        color: Colors.transparent,
        borderRadius: BorderRadius.circular(Radii.md),
        // The media carousel intentionally continues through the list's trailing
        // gutter. InkWell still clips its ink to [borderRadius], while leaving
        // overflowing message content visible.
        clipBehavior: Clip.none,
        child: MessageLongPressInkWell(
          key: ValueKey('message-row-${message.id}'),
          onLongPressDetails: openMessageActions,
          borderRadius: BorderRadius.circular(Radii.md),
          highlightColor: context.colors.primary.withValues(alpha: 0.1),
          snapshotKey: messageSnapshotKey,
          // Tap opens existing threads; long-press can start a new one.
          // MessageContent handles mention, channel-link, and media taps.
          onTap: (!hasReplies && !hasLocalReplies) || allMessages == null
              ? null
              : () => Navigator.of(context).push(
                  MaterialPageRoute<void>(
                    builder: (_) => ThreadDetailPage(
                      threadHead: message,
                      allMessages: allMessages!,
                      channelId: currentChannelId,
                      currentPubkey: currentPubkey,
                      isMember: isMember,
                      isArchived: isArchived,
                    ),
                  ),
                ),
          child: Padding(
            padding: EdgeInsets.only(
              top: showAuthor ? 0 : Grid.quarter,
              bottom: showAuthor ? 0 : Grid.quarter,
            ),
            child: RepaintBoundary(
              key: messageSnapshotKey,
              child: ChatBubbleRow(
                bare: watchMessageIsEmojiOnly(
                  ref,
                  content: message.content,
                  tags: message.tags,
                ),
                bubbleKey: ValueKey('message-bubble-${message.id}'),
                isOwn: isOwnMessage,
                showAuthor: showAuthor,
                createdAt: message.createdAt,
                edited: message.edited,
                timestampKey: ValueKey('message-timestamp-${message.id}'),
                avatar: GestureDetector(
                  onTap: () => showUserProfileSheet(
                    context,
                    message.pubkey,
                    names: channelIdentityNamesProvider(currentChannelId),
                  ),
                  child: AgentAvatarBadge(
                    badge: watchAgentBadge(
                      ref,
                      agentPubkey: message.pubkey,
                      ownerPubkey: profile?.ownerPubkey,
                      viewerPubkey: currentPubkey,
                    ),
                    child: _UserAvatar(
                      profile: profile,
                      pubkey: message.pubkey,
                      isAgent: isAgent,
                    ),
                  ),
                ),
                header: showAuthor && !isOwnMessage
                    ? ChatBubbleAuthor(
                        key: ValueKey('message-author-${message.id}'),
                        displayName: displayName,
                        onTap: () => showUserProfileSheet(
                          context,
                          message.pubkey,
                          names: channelIdentityNamesProvider(currentChannelId),
                        ),
                      )
                    : null,
                content: [
                  MessageQuoteHeader(
                    channelId: currentChannelId,
                    tags: message.tags,
                    loadedMessages: allMessages,
                  ),
                  ReadAloudMessage(
                    messageId: message.id,
                    content: message.content,
                    child: MessageContent(
                      content: message.content,
                      mentionNames: resolvedMentionNames,
                      mentionLabels: mentionLabels,
                      agentMentionPubkeys: agentMentionPubkeys,
                      channelNames: channelNames,
                      tags: message.tags,
                      baseStyle: messageBodyTextStyle.copyWith(
                        color: context.colors.onSurface,
                      ),
                      scaleEmojiOnly: true,
                      mediaCarouselTrailingOverflow: Grid.gutter,
                      onMediaReply: allMessages == null
                          ? null
                          : () {
                              if (!context.mounted) return;
                              Navigator.of(context).push(
                                MaterialPageRoute<void>(
                                  builder: (_) => ThreadDetailPage(
                                    threadHead: message,
                                    allMessages: allMessages!,
                                    channelId: currentChannelId,
                                    currentPubkey: currentPubkey,
                                    isMember: isMember,
                                    isArchived: isArchived,
                                  ),
                                ),
                              );
                            },
                      onMediaMore: (viewerContext, imageUrl) =>
                          showImageActions(
                            context: viewerContext,
                            ref: ref,
                            message: message,
                            channelId: currentChannelId,
                            imageUrl: imageUrl,
                            canManageMessage: canManageMessage,
                            onDeleted: () {
                              if (viewerContext.mounted) {
                                Navigator.of(viewerContext).maybePop();
                              }
                            },
                          ),
                      onChannelTap: (channelId) {
                        openChannelLink(
                          context: context,
                          ref: ref,
                          channelId: channelId,
                          currentChannelId: currentChannelId,
                        );
                      },
                      onMentionTap: (pubkey) => showUserProfileSheet(
                        context,
                        pubkey,
                        names: channelIdentityNamesProvider(currentChannelId),
                      ),
                    ),
                  ),
                ],
                below: message.reactions.isEmpty
                    ? null
                    : ReactionRow(
                        messageId: message.id,
                        channelId: currentChannelId,
                        reactions: message.reactions,
                        onToggle: (emoji) =>
                            toggleReaction(ref, message, emoji),
                        showAddButton: isMember && !isArchived,
                        onAddReaction: () => showAddReactionPicker(
                          context: context,
                          ref: ref,
                          message: message,
                        ),
                      ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _UserAvatar extends StatelessWidget {
  final UserProfile? profile;
  final String pubkey;
  final bool isAgent;

  const _UserAvatar({
    required this.profile,
    required this.pubkey,
    required this.isAgent,
  });

  static const size = messageAvatarSize;

  @override
  Widget build(BuildContext context) {
    final initial =
        profile?.initial ?? (pubkey.isNotEmpty ? pubkey[0].toUpperCase() : '?');
    final avatarUrl = profile?.avatarUrl;

    return AvatarImage(
      imageUrl: avatarUrl,
      radius: size / 2,
      backgroundColor: context.colors.primaryContainer,
      fallback: Text(
        initial,
        style:
            (size > 28
                    ? context.textTheme.labelMedium
                    : context.textTheme.labelSmall)
                ?.copyWith(
                  color: context.colors.onPrimaryContainer,
                  fontWeight: FontWeight.w600,
                ),
      ),
      isAgent: isAgent,
    );
  }
}

IconData channelIcon(Channel channel) {
  if (channel.isDm) return LucideIcons.messagesSquare;
  if (channel.isPrivate) return LucideIcons.lock;
  if (channel.isForum) return LucideIcons.messageSquareText;
  return LucideIcons.hash;
}
