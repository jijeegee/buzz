import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/identity_names/identity_names_provider.dart';
import '../../shared/l10n/l10n.dart';
import '../activity/inbox_item.dart';
import '../channels/channel.dart';
import '../channels/channel_management_provider.dart';
import '../channels/channels_provider.dart';
import '../channels/channel_names/channel_names_provider.dart';
import '../channels/group_chat/chat_title.dart';
import '../chats/chat_list.dart';
import '../chats/chats_page.dart';
import 'search_provider.dart';

/// Characters a word starts after in a chat name.
const _wordBreaks = {' ', '-', '_', '#', '.', '/', '(', '[', '·', '›'};

/// How well [name] matches [query], case-insensitive: 0 exact, 1 starts
/// with, 2 a word in it starts with it, 3 contains it. Null when it does not
/// match or the query is blank.
int? searchMatchRank(String name, String query) {
  final q = query.trim().toLowerCase();
  if (q.isEmpty) return null;
  final n = name.trim().toLowerCase();
  if (n == q) return 0;
  if (n.startsWith(q)) return 1;
  var index = n.isEmpty ? -1 : n.indexOf(q, 1);
  if (index < 0) return null;
  while (index > 0) {
    if (_wordBreaks.contains(n[index - 1])) return 2;
    index = n.indexOf(q, index + 1);
  }
  return 3;
}

/// A chat found by name: a group, a DM, or a thread in either.
@immutable
class SearchChatResult {
  const SearchChatResult({
    required this.channelId,
    required this.channelName,
    required this.title,
    required this.at,
    this.channel,
    this.thread,
  });

  final String channelId;

  /// Null when the thread's channel is not loaded on this device.
  final Channel? channel;

  /// The Chats thread row, for a thread result.
  final ChatListRow? thread;

  /// The group's name or the DM's people.
  final String channelName;

  /// What matched: the group or DM name, or the thread's title.
  final String title;

  /// Last activity, unix seconds.
  final int at;

  bool get isThread => thread != null;
  String get key => thread?.key ?? channelId;
}

/// Chats matched by name: groups and their threads first, then DMs.
@immutable
class SearchChatResults {
  const SearchChatResults({this.groups = const [], this.dms = const []});

  final List<SearchChatResult> groups;
  final List<SearchChatResult> dms;

  bool get isEmpty => groups.isEmpty && dms.isEmpty;
}

/// Chats whose name matches [query]. Groups hold channels and the threads in
/// them; DMs hold existing DMs and their threads. Each list is sorted by
/// match (exact, starts with, word start, middle), then most recent activity.
/// Groups this account has not joined come last. A group matched only by
/// its description ranks after every name match.
SearchChatResults buildSearchChatResults({
  required String query,
  required List<ChatListRow> rows,
  required List<Channel> channels,
  required String Function(Channel channel) dmLabel,
}) {
  if (query.trim().isEmpty) return const SearchChatResults();
  final channelById = {for (final channel in channels) channel.id: channel};
  final roomAt = {
    for (final row in rows)
      if (!row.isThread) row.channelId: row.at,
  };

  final groups = <({SearchChatResult result, int tier, int rank})>[];
  final dms = <({SearchChatResult result, int tier, int rank})>[];

  for (final channel in channels) {
    // Chats of people are listed with DMs, by their title.
    if (channel.isPersonChat) continue;
    final rank =
        searchMatchRank(channel.name, query) ??
        (searchMatchRank(channel.description, query) == null ? null : 4);
    if (rank == null) continue;
    final at =
        roomAt[channel.id] ??
        (channel.lastMessageAt?.millisecondsSinceEpoch ?? 0) ~/ 1000;
    groups.add((
      result: SearchChatResult(
        channelId: channel.id,
        channel: channel,
        channelName: channel.name,
        title: channel.name,
        at: at,
      ),
      tier: channel.isMember ? 0 : 1,
      rank: rank,
    ));
  }

  final dmRoomIds = <String>{};
  for (final row in rows) {
    if (!row.isPersonChat || row.isThread || !dmRoomIds.add(row.channelId)) {
      continue;
    }
    final channel = channelById[row.channelId];
    final label = channel == null ? row.channelName : dmLabel(channel);
    final rank = searchMatchRank(label, query);
    if (rank == null) continue;
    dms.add((
      result: SearchChatResult(
        channelId: row.channelId,
        channel: channel,
        channelName: label,
        title: label,
        at: row.at,
      ),
      tier: 0,
      rank: rank,
    ));
  }

  for (final row in rows) {
    if (!row.isThread) continue;
    final title = chatThreadTitle(row);
    final rank = searchMatchRank(title, query);
    if (rank == null) continue;
    final channel = channelById[row.channelId];
    final channelName = channel == null
        ? row.channelName.trim()
        : row.isPersonChat
        ? dmLabel(channel)
        : channel.name;
    final result = SearchChatResult(
      channelId: row.channelId,
      channel: channel,
      thread: row,
      channelName: channelName.isEmpty
          ? (row.isDm
                ? appL10n.searchDirectMessage
                : row.isGroupChat
                ? emptyChatTitle
                : appL10n.searchGroup)
          : channelName,
      title: title,
      at: row.at,
    );
    (row.isPersonChat ? dms : groups).add((
      result: result,
      tier: 0,
      rank: rank,
    ));
  }

  List<SearchChatResult> sorted(
    List<({SearchChatResult result, int tier, int rank})> entries,
  ) {
    entries.sort((a, b) {
      final byTier = a.tier.compareTo(b.tier);
      if (byTier != 0) return byTier;
      final byRank = a.rank.compareTo(b.rank);
      if (byRank != 0) return byRank;
      return b.result.at.compareTo(a.result.at);
    });
    return [for (final entry in entries) entry.result];
  }

  return SearchChatResults(groups: sorted(groups), dms: sorted(dms));
}

/// The current query's chats by name, from the Chats list and the channels
/// on this device. Updates as those load.
final searchChatResultsProvider = Provider<SearchChatResults>((ref) {
  final query = ref.watch(searchProvider.select((state) => state.query));
  if (query.isEmpty) return const SearchChatResults();
  final currentPubkey = ref.watch(currentPubkeyProvider);
  final names = ref.watch(identityNameSourcesProvider);
  final privateNames = ref.watch(channelNamesProvider);
  return buildSearchChatResults(
    query: query,
    rows: ref.watch(chatsRowsProvider) ?? const [],
    channels: ref.watch(channelsProvider).value ?? const [],
    dmLabel: (channel) => channelTitle(
      channel,
      currentPubkey: currentPubkey,
      names: names,
      privateName: privateNames.nameFor(channel.id),
    ),
  );
});

/// The thread a message search hit was posted in, or null for a message on
/// a main timeline (including broadcast replies) and for forum events.
({String rootId, String? parentId})? searchHitThread(SearchHit hit) {
  if (hit.kind != 9 && hit.kind != 40002) return null;
  if (!isThreadReply(hit.tags)) return null;
  final reference = threadReferenceOf(hit.tags);
  return (rootId: reference.rootId!, parentId: reference.parentId);
}

/// The Chats title of the thread [thread] points at, or 'Thread' when that
/// thread is not in the loaded list.
String searchHitThreadTitle(
  ({String rootId, String? parentId}) thread,
  List<ChatListRow> rows,
) {
  for (final row in rows) {
    if (row.isThread &&
        (row.rootId == thread.rootId || row.key == thread.rootId)) {
      return chatThreadTitle(row);
    }
  }
  for (final row in rows) {
    if (row.isThread &&
        thread.parentId != null &&
        row.rootId == thread.parentId) {
      return chatThreadTitle(row);
    }
  }
  return appL10n.searchThread;
}
