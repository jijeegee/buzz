import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/identity_names/identity_names_provider.dart';
import '../../shared/mentions/agent_identity_provider.dart';
import '../../shared/mentions/mention_tags.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/read_state/read_state_provider.dart';
import '../../shared/relay/relay.dart';
import '../../shared/theme/theme.dart';
import '../../shared/utils/string_utils.dart';
import '../../shared/widgets/bee_refresh_indicator.dart';
import '../../shared/widgets/conversation_row_parts.dart';
import '../../shared/widgets/frosted_app_bar.dart';
import '../../shared/widgets/frosted_scaffold.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../channels/channel.dart';
import '../channels/channel_detail_page.dart';
import '../channels/channel_management_provider.dart';
import '../channels/channel_mutes/channel_mutes_provider.dart';
import '../channels/channel_stars/channel_stars_provider.dart';
import '../channels/channels_page.dart';
import '../channels/channels_provider.dart';
import '../channels/dm_channel_labels.dart';
import '../people/blocked_users_provider.dart';
import '../search/search_page.dart';
import 'chat_list.dart';
import 'chat_list_provider.dart';
import 'thread_pins_provider.dart';

part 'chats_page/rows.dart';

/// All chats (every room and live thread, newest first) or By channel
/// (threads under their channel).
enum ChatsViewMode { all, byChannel }

const _viewModeKey = 'kuzz.chats.viewMode';

class ChatsViewModeNotifier extends Notifier<ChatsViewMode> {
  @override
  ChatsViewMode build() =>
      ref.read(savedPrefsProvider).getString(_viewModeKey) == 'by-channel'
      ? ChatsViewMode.byChannel
      : ChatsViewMode.all;

  void set(ChatsViewMode mode) {
    state = mode;
    unawaited(
      ref
          .read(savedPrefsProvider)
          .setString(
            _viewModeKey,
            mode == ChatsViewMode.byChannel ? 'by-channel' : 'all',
          ),
    );
  }
}

final chatsViewModeProvider =
    NotifierProvider<ChatsViewModeNotifier, ChatsViewMode>(
      ChatsViewModeNotifier.new,
    );

/// The rows the Chats screen lists: the relay's rows (archived ones left
/// out) plus this device's rooms the relay did not send, at the end. When
/// the relay keeps no chat state, every room comes from this device.
///
/// With no list yet (first install or just signed in, nothing saved), this
/// device's rooms show newest first until the relay's list replaces them,
/// rather than a loading spinner. Null while there is nothing to show yet.
final chatsRowsProvider = Provider<List<ChatListRow>?>((ref) {
  final list = ref.watch(chatListProvider);
  final channels = ref.watch(channelsProvider).value;
  final failed = chatListFailed(list);
  final serverRows = failed ? const <ChatListRow>[] : list.value?.rows;
  final listed = {
    for (final row in serverRows ?? const <ChatListRow>[])
      if (!row.isThread) row.channelId,
  };
  final hiddenDms = ref.read(channelsProvider.notifier).hiddenDmIds;
  final localRooms = [
    for (final channel in channels ?? const <Channel>[])
      if (!listed.contains(channel.id) &&
          !channel.isArchived &&
          (channel.isDm || (channel.isStream && channel.isMember)))
        localChatRow(channel),
  ]..sort((a, b) => b.atUs.compareTo(a.atUs));
  if (serverRows == null) return localRooms.isEmpty ? null : localRooms;
  return [
    for (final row in serverRows)
      if (!row.archived &&
          !(row.isDm && !row.isThread && hiddenDms.contains(row.channelId)))
        row,
    ...localRooms,
  ];
});

/// A room row for a channel the relay did not list.
ChatListRow localChatRow(Channel channel) {
  final at = (channel.lastMessageAt?.millisecondsSinceEpoch ?? 0) ~/ 1000;
  return ChatListRow(
    isThread: false,
    key: channel.id,
    at: at,
    atUs: at * 1000000,
    channelId: channel.id,
    channelType: channel.channelType,
    channelName: channel.name,
    dmMembers: channel.participantPubkeys,
  );
}

/// Shown unread state of one row, after this device's read position.
ChatRowUnread watchChatRowUnread(WidgetRef ref, ChatListRow row) {
  final readAt = ref.watch(
    readStateProvider.select(
      (state) => state.effectiveTimestamp(chatRowContextId(row)),
    ),
  );
  return chatRowUnread(row, readAt);
}

/// Whether any unmuted chat has unread messages (the Chats tab dot).
final chatsHaveUnreadProvider = Provider<bool>((ref) {
  final rows = ref.watch(chatsRowsProvider);
  if (rows == null) return false;
  final readState = ref.watch(readStateProvider);
  final mutes = ref.watch(channelMutesProvider).store.channels;
  return rows.any(
    (row) =>
        mutes[row.channelId]?.muted != true &&
        chatRowUnread(
          row,
          readState.effectiveTimestamp(chatRowContextId(row)),
        ).isUnread,
  );
});

class ChatsPage extends HookConsumerWidget {
  const ChatsPage({this.tabReselection, super.key});

  /// Notifies this page when its already-selected tab is tapped again.
  final ValueListenable<int>? tabReselection;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final list = ref.watch(chatListProvider);
    final rows = ref.watch(chatsRowsProvider);
    final mode = ref.watch(chatsViewModeProvider);
    final channels = ref.watch(channelsProvider).value ?? const <Channel>[];
    final channelById = {for (final channel in channels) channel.id: channel};
    final currentPubkey = ref.watch(currentPubkeyProvider);
    final starred = {
      for (final entry
          in ref.watch(channelStarsProvider).store.channels.entries)
        if (entry.value.starred) entry.key,
    };
    final muted = {
      for (final entry
          in ref.watch(channelMutesProvider).store.channels.entries)
        if (entry.value.muted) entry.key,
    };
    final pinnedThreads = ref.watch(threadPinsProvider);
    final readState = ref.watch(readStateProvider);
    final expanded = useState(<String>{});
    final scrollController = useScrollController();
    final reducedMotion = MediaQuery.disableAnimationsOf(context);
    useEffect(() {
      final tabReselection = this.tabReselection;
      if (tabReselection == null) return null;
      void scrollToTop() {
        if (!scrollController.hasClients) return;
        if (reducedMotion) {
          scrollController.jumpTo(0);
          return;
        }
        unawaited(
          scrollController.animateTo(
            0,
            duration: const Duration(milliseconds: 260),
            curve: Curves.easeOutCubic,
          ),
        );
      }

      tabReselection.addListener(scrollToTop);
      return () => tabReselection.removeListener(scrollToTop);
    }, [tabReselection, scrollController, reducedMotion]);

    final headerTitleStyle = context.textTheme.titleMedium?.copyWith(
      fontSize: 22,
      fontWeight: FontWeight.w600,
      color: navigationPrimaryForeground(context),
    );
    const switchHeight = 36.0;
    const bottomHeight = switchHeight + Grid.xs;
    final topSectionHeight = frostedAppBarHeight(
      context,
      titleStyle: headerTitleStyle,
      bottomHeight: bottomHeight,
    );

    Future<void> openRow(ChatListRow row) async {
      final channel = channelById[row.channelId];
      if (channel == null) {
        ScaffoldMessenger.maybeOf(context)?.showSnackBar(
          const SnackBar(content: Text('This chat is still loading.')),
        );
        return;
      }
      unawaited(HapticFeedback.selectionClick());
      await Navigator.of(context).push(
        MaterialPageRoute<void>(
          builder: (_) => ChannelDetailPage(
            channel: channel,
            initialThreadRootId: row.isThread ? row.rootId : null,
            initialThreadRouteBehavior:
                InitialThreadRouteBehavior.replaceCurrentRoute,
          ),
        ),
      );
    }

    Future<void> openNewMessage() async {
      final opened = await showNewMessageSheet(
        context,
        currentPubkey: currentPubkey,
      );
      if (opened == null || !context.mounted) return;
      await Navigator.of(context).push(
        MaterialPageRoute<void>(
          builder: (_) => ChannelDetailPage(channel: opened),
        ),
      );
    }

    // Pull to refresh waits for the Chats list only; the channel list
    // reloads behind it.
    Future<void> refresh() async {
      ref.read(channelsProvider.notifier).refresh().ignore();
      await refreshChatList(ref);
    }

    int unreadOf(ChatListRow row) {
      final unread = chatRowUnread(
        row,
        readState.effectiveTimestamp(chatRowContextId(row)),
      );
      return unread.count;
    }

    final Widget body;
    if (rows == null) {
      body = list.hasError && !chatListFailed(list)
          ? const SizedBox.shrink()
          : const Center(child: KuzzLoadingIndicator());
    } else if (rows.isEmpty) {
      body = Padding(
        padding: EdgeInsets.only(top: topSectionHeight),
        child: _ChatsEmptyState(
          onFindChannels: () => unawaited(showBrowseChannelsSheet(context)),
          onMessageSomeone: () => unawaited(openNewMessage()),
        ),
      );
    } else {
      final ordered = orderChatRows(
        rows,
        starred,
        pinnedThreadIds: pinnedThreads,
      );
      final entries = mode == ChatsViewMode.byChannel
          ? groupChatRowsByChannel(
              ordered,
              expandedChannelIds: expanded.value,
              unreadOf: unreadOf,
              pinnedThreadIds: pinnedThreads,
            )
          : [for (final row in ordered) _AllEntry(row)];
      // Channels with an unread open thread: their room row shows a dot.
      final threadUnreadChannels = {
        for (final row in rows)
          if (row.isThread &&
              !row.closed &&
              chatRowUnread(
                row,
                readState.effectiveTimestamp(chatRowContextId(row)),
              ).isUnread)
            row.channelId,
      };
      bool pinnedRow(ChatListRow row) =>
          isPinnedChatRow(row, starred, pinnedThreads);
      body = BeeRefreshIndicator(
        edgeOffset: topSectionHeight,
        onRefresh: refresh,
        child: CustomScrollView(
          controller: scrollController,
          slivers: [
            SliverToBoxAdapter(child: SizedBox(height: topSectionHeight)),
            SliverPadding(
              padding: EdgeInsets.only(
                top: Grid.xxs,
                bottom: MediaQuery.paddingOf(context).bottom + Grid.xxs,
              ),
              sliver: SliverList.builder(
                itemCount: entries.length,
                itemBuilder: (context, index) {
                  final entry = entries[index];
                  return switch (entry) {
                    _AllEntry(:final row) => _ChatRow(
                      key: ValueKey('chat-row-${row.key}'),
                      row: row,
                      channel: channelById[row.channelId],
                      currentPubkey: currentPubkey,
                      showChannelInTitle: true,
                      indent: false,
                      pinned: pinnedRow(row),
                      muted: muted.contains(row.channelId),
                      threadUnread: threadUnreadChannels.contains(
                        row.channelId,
                      ),
                      onTap: () => unawaited(openRow(row)),
                    ),
                    ChatGroupRoom(:final channelId, :final row) =>
                      row == null
                          ? _ChannelHeaderRow(
                              key: ValueKey('chat-channel-$channelId'),
                              channel: channelById[channelId],
                              fallbackName:
                                  _firstThreadOf(entries, index)?.channelName ??
                                  '',
                            )
                          : _ChatRow(
                              key: ValueKey('chat-row-${row.key}'),
                              row: row,
                              channel: channelById[row.channelId],
                              currentPubkey: currentPubkey,
                              showChannelInTitle: true,
                              indent: false,
                              pinned: pinnedRow(row),
                              muted: muted.contains(row.channelId),
                              threadUnread: threadUnreadChannels.contains(
                                row.channelId,
                              ),
                              onTap: () => unawaited(openRow(row)),
                            ),
                    ChatGroupThread(:final row) => _ChatRow(
                      key: ValueKey('chat-row-${row.key}'),
                      row: row,
                      channel: channelById[row.channelId],
                      currentPubkey: currentPubkey,
                      showChannelInTitle: false,
                      indent: true,
                      pinned: pinnedRow(row),
                      muted: muted.contains(row.channelId),
                      threadUnread: false,
                      onTap: () => unawaited(openRow(row)),
                    ),
                    ChatGroupMore() => _MoreThreadsRow(
                      key: ValueKey('chat-more-${entry.channelId}'),
                      more: entry,
                      onTap: () {
                        final next = {...expanded.value};
                        if (!next.remove(entry.channelId)) {
                          next.add(entry.channelId);
                        }
                        expanded.value = next;
                      },
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

    return FrostedScaffold(
      backgroundColor: conversationListBackground(context),
      appBar: FrostedAppBar(
        automaticallyImplyLeading: false,
        horizontalInset: Grid.gutter,
        showBottomDivider: true,
        bottomDividerOpacity: 0.07,
        title: Text('Chats', style: headerTitleStyle),
        titleStyle: headerTitleStyle,
        actions: [
          IconButton(
            key: const ValueKey('chats-browse-channels'),
            tooltip: 'Browse channels',
            icon: const Icon(LucideIcons.hash, size: 20),
            onPressed: () => unawaited(showBrowseChannelsSheet(context)),
          ),
          IconButton(
            key: const ValueKey('chats-search'),
            tooltip: 'Search',
            icon: const Icon(LucideIcons.search, size: 20),
            onPressed: () => Navigator.of(
              context,
            ).push(MaterialPageRoute<void>(builder: (_) => const SearchPage())),
          ),
        ],
        bottomHeight: bottomHeight,
        bottom: Padding(
          padding: const EdgeInsets.fromLTRB(
            Grid.gutter,
            0,
            Grid.gutter,
            Grid.xs,
          ),
          child: SizedBox(
            height: switchHeight,
            child: _ViewModeSwitch(
              mode: mode,
              onChanged: (next) =>
                  ref.read(chatsViewModeProvider.notifier).set(next),
            ),
          ),
        ),
      ),
      body: body,
    );
  }
}

/// An All chats entry (one row).
final class _AllEntry extends ChatGroupEntry {
  const _AllEntry(this.row);

  final ChatListRow row;
}

ChatListRow? _firstThreadOf(List<ChatGroupEntry> entries, int index) {
  final next = index + 1 < entries.length ? entries[index + 1] : null;
  return next is ChatGroupThread ? next.row : null;
}
