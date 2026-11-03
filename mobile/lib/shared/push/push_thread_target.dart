/// The thread a push notification opens, shared by the Android notification
/// builder and the tap handler.
library;

final _eventId = RegExp(r'^[0-9a-f]{64}$');

/// Whether [value] is a lowercase hex Nostr event id.
bool isPushEventId(Object? value) =>
    value is String && _eventId.hasMatch(value);

/// The message whose thread holds an event with these [tags], or null for a
/// message on the room's main timeline.
///
/// Mirrors `threadReferenceOf` and `isBroadcastReply`: a reply opens in the
/// thread of its `reply` parent (the inbox does the same), unless it is
/// broadcast to the main timeline.
String? pushThreadRootId(List<dynamic> tags) {
  String? parent;
  for (final tag in tags) {
    if (tag is! List || tag.length < 2) continue;
    if (tag[0] == 'broadcast' && tag[1] == '1') return null;
    if (tag.length >= 4 && tag[0] == 'e' && tag[3] == 'reply') {
      parent = '${tag[1]}';
    }
  }
  return isPushEventId(parent) ? parent : null;
}

/// The head of the thread holding an event with these [tags], which keys its
/// Chats row and name: the `root` tag, else the `reply` parent. Null for a
/// message on the room's main timeline.
String? pushThreadHeadId(List<dynamic> tags) {
  final parent = pushThreadRootId(tags);
  if (parent == null) return null;
  for (final tag in tags) {
    if (tag is List &&
        tag.length >= 4 &&
        tag[0] == 'e' &&
        tag[3] == 'root' &&
        isPushEventId(tag[1])) {
      return tag[1] as String;
    }
  }
  return parent;
}
