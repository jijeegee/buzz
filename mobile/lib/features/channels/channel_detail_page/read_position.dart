part of '../channel_detail_page.dart';

/// The time an open room is read up to: its newest top-level row.
int? _channelReadTimestamp({
  required Channel channel,
  required AsyncValue<List<NostrEvent>> messagesState,
  List<NostrEvent>? heldNewerEvents,
}) {
  if (channel.isForum) {
    return dateTimeToUnixSeconds(channel.lastMessageAt);
  }

  final events = messagesState.value;
  // A window opened at a message stops short of the present; opening the
  // channel still reads everything up to its newest known row.
  if (heldNewerEvents != null) {
    var latest = dateTimeToUnixSeconds(channel.lastMessageAt) ?? 0;
    for (final event in [...?events, ...heldNewerEvents]) {
      if (event.threadReference.parentId != null) continue;
      if (event.createdAt > latest) latest = event.createdAt;
    }
    return latest > 0 ? latest : null;
  }
  if (events != null && events.isNotEmpty) {
    var latest = 0;
    for (final event in events) {
      if (event.threadReference.parentId != null) continue;
      if (event.createdAt > latest) {
        latest = event.createdAt;
      }
    }
    if (latest > 0) {
      return latest;
    }
  }

  return dateTimeToUnixSeconds(channel.lastMessageAt);
}
