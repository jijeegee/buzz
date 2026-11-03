/// Names a notification like its Chats row (`channelTitle`, `chatThreadTitle`)
/// from what the wake engine fetched: it has no channel list, no profile cache
/// and no localized words, so anything it cannot name falls back to the sender.
library;

import 'dart:math';

import '../utils/string_utils.dart';

/// Other members named in a title before "+N".
const pushTitleMemberLimit = 3;

/// A room as its kind:39000 describes it.
class PushRoom {
  final String name;
  final bool isDm;
  final bool isGroupChat;

  /// The kind:39000 `p` tags, lowercase: a DM's members.
  final List<String> dmMembers;

  const PushRoom({
    required this.name,
    required this.isDm,
    required this.isGroupChat,
    required this.dmMembers,
  });

  /// `null` for an event that is not a room's metadata.
  static PushRoom? fromMetadata(Map<String, dynamic>? event) {
    if (event == null || event['kind'] != 39000) return null;
    var name = '';
    var type = '';
    var groupChat = false;
    final members = <String>[];
    for (final tag in event['tags'] as List) {
      if (tag is! List || tag.isEmpty) continue;
      // A bare one-element marker.
      if (tag[0] == 'group_chat') groupChat = true;
      if (tag.length < 2) continue;
      switch (tag[0]) {
        case 'name':
          name = '${tag[1]}'.trim();
        case 't':
          type = '${tag[1]}';
        case 'p':
          final pubkey = '${tag[1]}'.toLowerCase();
          if (!members.contains(pubkey)) members.add(pubkey);
      }
    }
    final isDm = type == 'dm';
    return PushRoom(
      name: name,
      isDm: isDm,
      isGroupChat: groupChat && !isDm,
      dmMembers: isDm ? members : const [],
    );
  }

  /// A DM of exactly two people: titled by the sender.
  bool get isOneToOne => isDm && dmMembers.length == 2;

  /// Titled by its other members: a group DM (its name is generic) or an
  /// unnamed group chat.
  bool get namedByMembers =>
      (isDm && !isOneToOne) || (isGroupChat && name.isEmpty);
}

/// [pubkeys] without [me], lowercase, in the order given.
List<String> pushOtherMembers(Iterable<String> pubkeys, String? me) {
  final mine = me?.toLowerCase();
  final seen = <String>{};
  return [
    for (final pubkey in pubkeys)
      if (pubkey.toLowerCase() != mine && seen.add(pubkey.toLowerCase()))
        pubkey.toLowerCase(),
  ];
}

/// "Ann, Bob, Cy +2" over [others], with unknown people by short key. Empty
/// when there is nobody else.
String pushMemberTitle(List<String> others, Map<String, String> names) {
  final shown = [
    for (final pubkey in others.take(pushTitleMemberLimit))
      names[pubkey] ?? shortPubkey(pubkey),
  ].join(', ');
  final hidden = others.length - min(others.length, pushTitleMemberLimit);
  return hidden > 0 ? '$shown +$hidden' : shown;
}

/// A thread's title: its name, else the root's first line, else the new
/// message's ([fallback]).
String pushThreadTitle({
  String? threadName,
  String? rootContent,
  required String fallback,
}) {
  final name = threadName?.trim();
  if (name != null && name.isNotEmpty) return name;
  final firstLine = rootContent
      ?.split('\n')
      .map((line) => line.trim())
      .where((line) => line.isNotEmpty)
      .firstOrNull;
  if (firstLine == null) return fallback;
  return firstLine.length > 80 ? '${firstLine.substring(0, 80)}…' : firstLine;
}

/// The notification's conversation title, or null when the sender alone
/// titles it: a 1:1 DM, or a room with no usable name.
String? pushConversationTitle({
  required PushRoom room,
  required String memberTitle,
  required String sender,
  required String? threadTitle,
}) {
  final String? name;
  if (room.isOneToOne) {
    name = null;
  } else if (room.namedByMembers) {
    name = memberTitle.isEmpty ? null : memberTitle;
  } else {
    name = room.name.isEmpty ? null : room.name;
  }
  if (threadTitle == null) return name;
  return '${name ?? sender} > $threadTitle';
}
