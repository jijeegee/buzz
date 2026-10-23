part of '../chats_page.dart';

/// `{name}'s conversations`: the agent's chats without me, in the Chats
/// list's rows (All chats | By group, threads under their chat), tighter and
/// grey, tagged `Agent` / `Person` / `N people`, with no counts (the relay
/// sends no read state here). A chat opens read-only.
class AgentConversationsList extends HookConsumerWidget {
  const AgentConversationsList({
    super.key,
    required this.agentPubkey,
    required this.agentName,
    required this.dmChannelId,
    required this.topInset,
  });

  final String agentPubkey;
  final String agentName;

  /// The owner's DM with the agent, which hosts this list.
  final String dmChannelId;

  /// Height of the header over the list.
  final double topInset;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final agent = agentPubkey.toLowerCase();
    final list = ref.watch(agentChatRowsProvider(agent));
    final mode = useState(ChatsViewMode.all);
    final expanded = useState(<String, int>{});
    final knownAgents = ref.watch(knownAgentPubkeysProvider);
    final profiles = ref.watch(userCacheProvider);
    final background = ownerViewBackground(context);
    final rows = list.value;

    Future<void> refresh() async {
      ref.invalidate(agentChatListProvider(agent));
      try {
        await ref.read(agentChatListProvider(agent).future);
      } catch (error) {
        debugPrint('[OwnerView] chat list failed: $error');
      }
    }

    final Widget body;
    if (rows == null) {
      body = Padding(
        padding: EdgeInsets.only(top: topInset),
        child: list.hasError
            ? LoadErrorView(
                message: context.l10n.ownerViewLoadFailed,
                onRetry: () => ref.invalidate(agentChatListProvider(agent)),
              )
            : const Center(child: KuzzLoadingIndicator()),
      );
    } else if (rows.isEmpty) {
      body = Padding(
        padding: EdgeInsets.only(top: topInset),
        child: Center(
          child: Padding(
            padding: const EdgeInsets.all(Grid.lg),
            child: Text(
              context.l10n.ownerViewEmpty(agentName),
              key: const ValueKey('agent-conversations-empty'),
              textAlign: TextAlign.center,
              style: context.textTheme.bodyMedium?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
          ),
        ),
      );
    } else {
      final channels = agentChatChannels(rows, agent);
      final ordered = orderChatRows(rows, const {});
      final entries = mode.value == ChatsViewMode.byChannel
          ? groupChatRowsByChannel(
              ordered,
              expandedSteps: expanded.value,
              unreadOf: (_) => 0,
            )
          : [for (final row in ordered) _AllEntry(row)];

      String? tagOf(String channelId) {
        final channel = channels[channelId];
        if (channel == null) return null;
        if (channel.isGroupChat) {
          return context.l10n.chatInfoPeopleCount(channel.memberCount);
        }
        if (!channel.isDm) return null;
        // The agent comes first; the tag is about who it talks to.
        final other = channel.participantPubkeys.skip(1).firstOrNull;
        if (other == null) return null;
        return knownAgents.contains(other) || profiles[other]?.isAgent == true
            ? context.l10n.ownerViewTagAgent
            : context.l10n.ownerViewTagPerson;
      }

      Future<void> open(ChatListRow row) async {
        final channel = channels[row.channelId];
        if (channel == null) return;
        unawaited(HapticFeedback.selectionClick());
        // Opening clears the dot; the page keeps it clear while open.
        final latest = row.latest;
        if (latest != null) {
          ref
              .read(ownerViewSeenProvider(agent).notifier)
              .markSeen(chatRowContextId(row), latest.createdAt);
        }
        await Navigator.of(context).push(
          MaterialPageRoute<void>(
            builder: (_) => ChannelDetailPage(
              channel: channel,
              ownerView: OwnerView(
                agentPubkey: agent,
                agentName: agentName,
                dmChannelId: dmChannelId,
              ),
              initialThreadRootId: row.isThread ? row.rootId : null,
              initialThreadRouteBehavior:
                  InitialThreadRouteBehavior.replaceCurrentRoute,
            ),
          ),
        );
      }

      Widget chatRow(
        ChatListRow row, {
        required bool showChannelInTitle,
        required bool indent,
      }) => _ChatRow(
        key: ValueKey('agent-chat-row-${row.key}'),
        row: row,
        channel: channels[row.channelId],
        currentPubkey: agent,
        showChannelInTitle: showChannelInTitle,
        indent: indent,
        pinned: false,
        muted: false,
        threadUnread: false,
        compact: true,
        tag: tagOf(row.channelId),
        background: background,
        onTap: () => unawaited(open(row)),
      );

      body = BeeRefreshIndicator(
        edgeOffset: topInset,
        onRefresh: refresh,
        child: CustomScrollView(
          key: const ValueKey('agent-conversations-list'),
          slivers: [
            SliverToBoxAdapter(child: SizedBox(height: topInset)),
            SliverToBoxAdapter(
              child: Padding(
                padding: const EdgeInsets.fromLTRB(
                  Grid.gutter,
                  Grid.xxs,
                  Grid.gutter,
                  Grid.half,
                ),
                child: SizedBox(
                  height: 36,
                  child: KuzzViewSwitch<ChatsViewMode>(
                    semanticsLabel: context.l10n.chatsViewLabel,
                    options: [
                      KuzzViewOption(
                        value: ChatsViewMode.all,
                        label: context.l10n.chatsViewAll,
                        key: const ValueKey('agent-chats-view-all'),
                      ),
                      KuzzViewOption(
                        value: ChatsViewMode.byChannel,
                        label: context.l10n.chatsViewByGroup,
                        key: const ValueKey('agent-chats-view-byChannel'),
                      ),
                    ],
                    selected: mode.value,
                    onChanged: (next) => mode.value = next,
                  ),
                ),
              ),
            ),
            SliverPadding(
              padding: EdgeInsets.only(
                bottom: MediaQuery.paddingOf(context).bottom + Grid.xxs,
              ),
              sliver: SliverList.builder(
                itemCount: entries.length,
                itemBuilder: (context, index) {
                  final entry = entries[index];
                  return switch (entry) {
                    _AllEntry(:final row) => chatRow(
                      row,
                      showChannelInTitle: true,
                      indent: false,
                    ),
                    ChatGroupRoom(:final channelId, :final row) =>
                      row == null
                          ? _ChannelHeaderRow(
                              key: ValueKey('agent-chat-channel-$channelId'),
                              channel: channels[channelId],
                              fallbackName:
                                  _firstThreadOf(entries, index)?.channelName ??
                                  '',
                            )
                          : chatRow(
                              row,
                              showChannelInTitle: true,
                              indent: false,
                            ),
                    ChatGroupThread(:final row) => chatRow(
                      row,
                      showChannelInTitle: false,
                      indent: true,
                    ),
                    ChatGroupMore() => _MoreThreadsRow(
                      key: ValueKey('agent-chat-more-${entry.channelId}'),
                      more: entry,
                      onShowMore: () => expanded.value = {
                        ...expanded.value,
                        entry.channelId:
                            (expanded.value[entry.channelId] ?? 0) + 1,
                      },
                      onShowLess: () =>
                          expanded.value = {...expanded.value}
                            ..remove(entry.channelId),
                    ),
                    _ => const SizedBox.shrink(),
                  };
                },
              ),
            ),
          ],
        ),
      );
    }

    return ColoredBox(color: background, child: body);
  }
}
