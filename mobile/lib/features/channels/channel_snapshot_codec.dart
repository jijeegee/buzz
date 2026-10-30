import 'dart:convert';

import 'channel.dart';

/// Version of the saved channel list; a list saved by another version is
/// ignored (the relay's list replaces it anyway).
const channelListSnapshotVersion = 1;

/// The saved channel list's key in the account's store file.
const channelListSnapshotKey = 'channels';

int? _ms(DateTime? at) => at?.millisecondsSinceEpoch;

DateTime? _at(Object? ms) =>
    ms is int ? DateTime.fromMillisecondsSinceEpoch(ms, isUtc: true) : null;

List<String> _strings(Object? value) => value is List
    ? [for (final item in value) ?(item is String ? item : null)]
    : const [];

Map<String, Object?> _channelJson(Channel channel) => {
  'id': channel.id,
  'name': channel.name,
  'type': channel.channelType,
  'visibility': channel.visibility,
  'description': channel.description,
  'topic': channel.topic,
  'purpose': channel.purpose,
  'created_by': channel.createdBy,
  'created_at': _ms(channel.createdAt),
  'member_count': channel.memberCount,
  'last_message_at': _ms(channel.lastMessageAt),
  'archived_at': _ms(channel.archivedAt),
  'participants': channel.participants,
  'participant_pubkeys': channel.participantPubkeys,
  'member': channel.isMember,
  'ttl_seconds': channel.ttlSeconds,
  'ttl_deadline': _ms(channel.ttlDeadline),
  'goals': channel.goalsEnabled,
  'picture': channel.picture,
  'group_chat': channel.groupChat,
  'history_all': channel.historyAll,
};

Channel? _channelFromJson(Object? json) {
  if (json is! Map<String, dynamic>) return null;
  final id = json['id'];
  final name = json['name'];
  final type = json['type'];
  final visibility = json['visibility'];
  if (id is! String || name is! String || type is! String) return null;
  if (visibility is! String) return null;
  return Channel(
    id: id,
    name: name,
    channelType: type,
    visibility: visibility,
    description: json['description'] is String
        ? json['description'] as String
        : '',
    topic: json['topic'] as String?,
    purpose: json['purpose'] as String?,
    createdBy: json['created_by'] is String ? json['created_by'] as String : '',
    createdAt:
        _at(json['created_at']) ??
        DateTime.fromMillisecondsSinceEpoch(0, isUtc: true),
    memberCount: json['member_count'] is int ? json['member_count'] as int : 0,
    lastMessageAt: _at(json['last_message_at']),
    archivedAt: _at(json['archived_at']),
    participants: _strings(json['participants']),
    participantPubkeys: _strings(json['participant_pubkeys']),
    isMember: json['member'] == true,
    ttlSeconds: json['ttl_seconds'] as int?,
    ttlDeadline: _at(json['ttl_deadline']),
    goalsEnabled: json['goals'] != false,
    picture: json['picture'] as String?,
    groupChat: json['group_chat'] == true,
    historyAll: json['history_all'] == true,
  );
}

/// [channels] as the saved channel list.
String encodeChannelListSnapshot(List<Channel> channels) => jsonEncode({
  'v': channelListSnapshotVersion,
  'channels': [for (final channel in channels) _channelJson(channel)],
});

/// The saved channel list, or `null` when [raw] is missing, from another
/// version or unreadable.
List<Channel>? decodeChannelListSnapshot(String? raw) {
  if (raw == null) return null;
  try {
    final json = jsonDecode(raw);
    if (json is! Map<String, dynamic> ||
        json['v'] != channelListSnapshotVersion) {
      return null;
    }
    final channels = json['channels'];
    if (channels is! List) return null;
    return [for (final item in channels) ?_channelFromJson(item)];
  } catch (_) {
    return null;
  }
}
