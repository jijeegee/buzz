import 'dart:convert';

import 'package:flutter/foundation.dart';

import '../relay/relay.dart';

/// Kind of the relay-signed overlay that closes a thread window. Like the
/// channel window's bounds (39006) it describes a response, not the room.
const _threadWindowBoundsKind = 39007;

/// One event as the message database stores it, classified on the isolate
/// that already holds the parsed [event]. The writer isolate only encodes
/// [event] and runs SQL; it never parses JSON.
@immutable
class MessageRow {
  const MessageRow({
    required this.event,
    required this.channelId,
    required this.receivedAt,
    this.stream,
    this.targetId,
    this.deleteTargets = const [],
  });

  final NostrEvent event;
  final String channelId;

  /// This device's clock, Unix seconds, when the event was received.
  final int receivedAt;

  /// `''` for a main-timeline row, the root id for a thread reply, and null
  /// for overlays (edits, reactions, summaries) and other kinds.
  final String? stream;

  /// The row an edit, reaction or thread summary belongs to.
  final String? targetId;

  /// Ids a deletion (kind 5 or 9005) removes; empty for every other kind.
  final List<String> deleteTargets;

  String get id => event.id;
  int get kind => event.kind;
  bool get isDeletion => deleteTargets.isNotEmpty;
  bool get isSummary => kind == EventKind.channelThreadSummary;

  /// The event as JSON, as the relay sent it.
  String encodeRaw() => jsonEncode(event.toJson());
}

/// [event] classified for storage, or null when it is not stored: it has no
/// room, or it is a window bounds overlay (a property of one response). A
/// page's events may lack an `h` tag; [channelId] then names their room.
MessageRow? messageRowFor(
  NostrEvent event, {
  required int receivedAt,
  String? channelId,
}) {
  final kind = event.kind;
  if (kind == EventKind.channelWindowBounds ||
      kind == _threadWindowBoundsKind) {
    return null;
  }
  final room = event.channelId ?? channelId;
  if (room == null) return null;
  String? stream;
  String? target;
  var deletes = const <String>[];
  if (EventKind.channelTimelineContentKinds.contains(kind)) {
    final thread = event.threadReference;
    stream = thread.parentId == null ? '' : thread.rootId ?? thread.parentId;
  } else if (kind == EventKind.deletion || kind == EventKind.nip29DeleteEvent) {
    deletes = _eTags(event).toSet().toList();
    // A deletion that names nothing changes nothing.
    if (deletes.isEmpty) return null;
  } else if (kind == EventKind.channelThreadSummary) {
    target = event.getTagValue('e');
    if (target == null) return null;
  } else if (kind == EventKind.streamMessageEdit ||
      kind == EventKind.reaction) {
    // The last `e` tag, as desktop reads edit and reaction targets.
    target = _eTags(event).lastOrNull;
  }
  return MessageRow(
    event: event,
    channelId: room,
    receivedAt: receivedAt,
    stream: stream,
    targetId: target,
    deleteTargets: deletes,
  );
}

/// [events] classified for storage, unstored ones skipped.
List<MessageRow> messageRowsFor(
  Iterable<NostrEvent> events, {
  required int receivedAt,
  String? channelId,
}) => [
  for (final event in events)
    ?messageRowFor(event, receivedAt: receivedAt, channelId: channelId),
];

Iterable<String> _eTags(NostrEvent event) sync* {
  for (final tag in event.tags) {
    if (tag.length >= 2 && tag[0] == 'e') yield tag[1];
  }
}
