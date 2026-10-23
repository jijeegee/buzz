import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../../shared/mentions/agent_identity_provider.dart';
import '../../../shared/profile/user_cache_provider.dart';
import '../../../shared/profile/user_profile.dart';
import '../../../shared/relay/relay.dart';
import '../../../shared/theme/theme.dart';
import '../../channels/channel.dart';
import '../../channels/channel_management_provider.dart';
import '../../channels/leave_with_agents.dart';
import '../../chats/chat_list.dart';
import '../../chats/chat_list_provider.dart';

/// Owner view: the owner of an agent reads, read-only, the agent's DMs and
/// group chats the owner is not in. The relay lists them (`chat_list` with
/// `as_agent`) with no read state, and lets the owner read their messages
/// and remove the agent. Mirrors desktop's `ownerView`.

/// Rows loaded per refresh; older chats stay out of the list.
const agentChatListMaxRows = 300;

/// A chat opened in the owner view: whose chat it is, and the owner's DM
/// with that agent (where Ask goes and Remove returns to).
@immutable
class OwnerView {
  const OwnerView({
    required this.agentPubkey,
    required this.agentName,
    required this.dmChannelId,
    this.chatLabel = '',
  });

  /// Lowercase hex pubkey of the agent whose chat this is.
  final String agentPubkey;
  final String agentName;
  final String dmChannelId;

  /// The chat's name, for Ask's draft and Remove's dialog.
  final String chatLabel;

  OwnerView withChatLabel(String chatLabel) => OwnerView(
    agentPubkey: agentPubkey,
    agentName: agentName,
    dmChannelId: dmChannelId,
    chatLabel: chatLabel,
  );

  @override
  bool operator ==(Object other) =>
      other is OwnerView &&
      other.agentPubkey == agentPubkey &&
      other.agentName == agentName &&
      other.dmChannelId == dmChannelId &&
      other.chatLabel == chatLabel;

  @override
  int get hashCode =>
      Object.hash(agentPubkey, agentName, dmChannelId, chatLabel);
}

/// The owner view a page shows, for the messages under it: the agent's
/// messages sit on the right and none can be managed.
class OwnerViewScope extends InheritedWidget {
  const OwnerViewScope({super.key, required this.view, required super.child});

  final OwnerView view;

  static OwnerView? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<OwnerViewScope>()?.view;

  @override
  bool updateShouldNotify(OwnerViewScope oldWidget) => view != oldWidget.view;
}

/// The grey behind an agent's conversations and the chats opened from them.
Color ownerViewBackground(BuildContext context) => Color.alphaBlend(
  context.colors.onSurface.withValues(alpha: 0.04),
  context.colors.surface,
);

/// Whether [agentPubkey]'s verified owner (profile NIP-OA tag or relay agent
/// directory) is [me].
bool isMyAgent({
  required String agentPubkey,
  required String? me,
  required Map<String, String> agentOwners,
  required Map<String, UserProfile> profiles,
}) => ownedAgentPubkeys(
  memberPubkeys: [agentPubkey],
  me: me,
  agentOwners: agentOwners,
  profiles: profiles,
).isNotEmpty;

/// [isMyAgent] for the signed-in account.
final isMyAgentProvider = Provider.autoDispose.family<bool, String>((
  ref,
  agentPubkey,
) {
  final pubkey = agentPubkey.toLowerCase();
  final profile = ref.watch(userCacheProvider.select((cache) => cache[pubkey]));
  return isMyAgent(
    agentPubkey: pubkey,
    me: ref.watch(currentPubkeyProvider),
    agentOwners: ref.watch(agentOwnersProvider).value ?? const {},
    profiles: {pubkey: ?profile},
  );
});

/// The other person of a one-to-one DM, lowercase; null for anything else.
String? oneToOneDmCounterpart(Channel channel, String? me) {
  if (!channel.isOneToOneDm) return null;
  final self = me?.toLowerCase();
  for (final pubkey in channel.participantPubkeys) {
    final normalized = pubkey.trim().toLowerCase();
    if (normalized.isNotEmpty && normalized != self) return normalized;
  }
  return null;
}

/// Which DM tab is shown with my agent: `Chat with X` or `X's conversations`.
enum AgentDmTab { chat, conversations }

class AgentDmTabNotifier extends Notifier<AgentDmTab> {
  AgentDmTabNotifier(this.channelId);

  final String channelId;

  @override
  AgentDmTab build() => AgentDmTab.chat;

  void set(AgentDmTab tab) => state = tab;
}

/// The tab shown in the DM [channelId] with one of my agents.
final agentDmTabProvider = NotifierProvider.autoDispose
    .family<AgentDmTabNotifier, AgentDmTab, String>(AgentDmTabNotifier.new);

/// A row without read state: the relay sends none here, so nothing counts.
ChatListRow _withoutReadState(ChatListRow row) => row.copyWith(
  unread: 0,
  unreadOverflow: false,
  unreadMentions: 0,
  unreadMentionsOverflow: false,
  unreadMark: false,
);

/// Loads pages until [maxRows] rows or the end of the list, each row once,
/// archived ones left out, read state stripped.
Future<List<ChatListRow>> loadAgentChatList(
  Future<ChatListPage> Function(ChatListCursor? before) loadPage, {
  int maxRows = agentChatListMaxRows,
}) async {
  final rows = <ChatListRow>[];
  final seen = <String>{};
  ChatListCursor? before;
  do {
    final page = await loadPage(before);
    for (final row in page.rows) {
      if (row.archived || !seen.add(row.key)) continue;
      rows.add(_withoutReadState(row));
    }
    before = page.next;
  } while (before != null && rows.length < maxRows);
  return rows;
}

/// [agentPubkey]'s chats without its owner, for the signed-in account.
final agentChatListProvider = FutureProvider.autoDispose
    .family<List<ChatListRow>, String>((ref, agentPubkey) async {
      // Another account or community never sees this list.
      ref.watch(chatListOwnerProvider);
      final session = ref.read(relaySessionProvider.notifier);
      return loadAgentChatList(
        (before) async => parseChatListResponse(
          await session.queryRelayJson([
            buildChatListFilter(
              limit: chatListPageSize,
              before: before,
              asAgent: agentPubkey,
            ),
          ], timeout: chatListQueryTimeout),
        ),
      );
    });

/// The chat a row belongs to, as a channel the owner can open read-only.
/// Its people put the agent first, then the others.
Channel channelFromAgentChatRow(ChatListRow row, String agentPubkey) {
  final agent = agentPubkey.toLowerCase();
  final people = [
    agent,
    for (final pubkey in row.dmMembers)
      if (pubkey.toLowerCase() != agent) pubkey.toLowerCase(),
  ];
  final latestAt = row.latest?.createdAt ?? (row.at > 0 ? row.at : null);
  return Channel(
    id: row.channelId,
    name: row.channelName,
    channelType: row.isDm ? 'dm' : 'stream',
    visibility: 'private',
    description: '',
    createdBy: '',
    createdAt: DateTime.fromMillisecondsSinceEpoch(0),
    memberCount: row.memberCount ?? people.length,
    lastMessageAt: latestAt == null
        ? null
        : DateTime.fromMillisecondsSinceEpoch(latestAt * 1000),
    participantPubkeys: people,
    groupChat: row.isGroupChat,
  );
}

/// One channel per chat, by channel id. DM thread rows carry no members, so
/// each chat is built from its row with the most members.
Map<String, Channel> agentChatChannels(
  Iterable<ChatListRow> rows,
  String agentPubkey,
) {
  final byId = <String, Channel>{};
  final memberCounts = <String, int>{};
  for (final row in rows) {
    if ((memberCounts[row.channelId] ?? -1) >= row.dmMembers.length) continue;
    memberCounts[row.channelId] = row.dmMembers.length;
    byId[row.channelId] = channelFromAgentChatRow(row, agentPubkey);
  }
  return byId;
}

/// An owner who can read a chat without being in it, through their agent.
typedef OwnerReader = ({String agentPubkey, String ownerPubkey});

/// Owners who can read [channel] without being in it: the owner of each
/// agent among [memberPubkeys], unless that owner is a member or [me]. Only
/// DMs and group chats are readable this way.
List<OwnerReader> ownerReaders({
  required Channel channel,
  required Iterable<String> memberPubkeys,
  required String? me,
  required Map<String, String> agentOwners,
  required Map<String, UserProfile> profiles,
}) {
  if (!channel.isPersonChat) return const [];
  final members = {for (final pubkey in memberPubkeys) pubkey.toLowerCase()};
  final self = me?.toLowerCase();
  return [
    for (final agent in members)
      if ((profiles[agent]?.ownerPubkey ?? agentOwners[agent])?.toLowerCase()
          case final owner? when owner != self && !members.contains(owner))
        (agentPubkey: agent, ownerPubkey: owner),
  ];
}

/// [ownerReaders] of [channel] from its loaded members.
List<OwnerReader> watchOwnerReaders(WidgetRef ref, Channel channel) {
  if (!channel.isPersonChat) return const [];
  final members = ref.watch(channelMembersProvider(channel.id)).value;
  if (members == null || members.isEmpty) return const [];
  return ownerReaders(
    channel: channel,
    memberPubkeys: members.map((member) => member.pubkey),
    me: ref.watch(currentPubkeyProvider),
    agentOwners: ref.watch(agentOwnersProvider).value ?? const {},
    profiles: ref.watch(userCacheProvider),
  );
}
