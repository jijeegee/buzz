import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:hooks_riverpod/misc.dart' show ProviderListenable;

import 'channel.dart';
import 'channel_management_provider.dart';
import 'channels_provider.dart';

// Live messages in any room replace that room's entry in [channelsProvider]
// (its `lastMessageAt`). A screen showing one room watches these slices so
// that traffic elsewhere does not rebuild it.

/// [channelId]'s entry in the channel list, or null while the list has no
/// value or does not hold it. Other rooms' updates keep the entry identical.
ProviderListenable<Channel?> listedChannel(String channelId) =>
    channelsProvider.select(
      (channels) => channels.value?.where((c) => c.id == channelId).firstOrNull,
    );

/// The cached member snapshot for [channelId], read whenever the channel list
/// changes. The snapshot list is replaced only when members are refetched.
ProviderListenable<List<ChannelMember>> cachedChannelMembers(
  ChannelsNotifier notifier,
  String channelId,
) => channelsProvider.select(
  (channels) => channels.asData == null
      ? const <ChannelMember>[]
      : notifier.cachedMembersForChannel(channelId),
);

/// The channel list ignoring activity times: it compares equal while no
/// channel changed anything but `lastMessageAt`.
final ProviderListenable<ChannelListShape> channelListShape = channelsProvider
    .select((channels) => ChannelListShape(channels.asData?.value));

/// A channel list compared by everything except `lastMessageAt`.
@immutable
class ChannelListShape {
  /// Null while the list has no value.
  final List<Channel>? channels;

  const ChannelListShape(this.channels);

  List<Channel> get list => channels ?? const <Channel>[];

  @override
  bool operator ==(Object other) {
    if (other is! ChannelListShape) return false;
    final a = channels, b = other.channels;
    if (identical(a, b)) return true;
    if (a == null || b == null || a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (!sameChannelIgnoringActivity(a[i], b[i])) return false;
    }
    return true;
  }

  @override
  int get hashCode => Object.hashAll([for (final channel in list) channel.id]);
}

/// Whether [a] and [b] differ at most in `lastMessageAt`. Every other
/// [Channel] field is compared; a new field must be added here.
bool sameChannelIgnoringActivity(Channel a, Channel b) =>
    identical(a, b) ||
    (a.id == b.id &&
        a.name == b.name &&
        a.channelType == b.channelType &&
        a.visibility == b.visibility &&
        a.description == b.description &&
        a.topic == b.topic &&
        a.purpose == b.purpose &&
        a.createdBy == b.createdBy &&
        a.createdAt == b.createdAt &&
        a.memberCount == b.memberCount &&
        a.archivedAt == b.archivedAt &&
        listEquals(a.participants, b.participants) &&
        listEquals(a.participantPubkeys, b.participantPubkeys) &&
        a.isMember == b.isMember &&
        a.ttlSeconds == b.ttlSeconds &&
        a.ttlDeadline == b.ttlDeadline &&
        a.goalsEnabled == b.goalsEnabled &&
        a.picture == b.picture &&
        a.groupChat == b.groupChat &&
        a.historyAll == b.historyAll);
