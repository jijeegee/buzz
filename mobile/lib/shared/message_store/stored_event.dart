/// Which received events the device message store keeps and how it files
/// them. The cases live in `test-fixtures/message-store.json`, shared with
/// desktop (`message_store/classify.rs`).
library;

import 'dart:convert';

/// Room content kinds: rows on a room timeline or in a thread.
const _contentKinds = {
  9, 40001, 40002, 40008, 40099, 43001, 43002, 43003, 43004, 43005, 43006, //
  45001, 45003, 48100, 48101, 48102, 48103,
};

/// Overlays on another event, kept by the ids they reference.
const _auxKinds = {5, 7, 9005, 40003};
const _threadSummary = 39005;

sealed class StoredEvent {
  const StoredEvent();
}

class StoredRow extends StoredEvent {
  const StoredRow({
    required this.id,
    required this.kind,
    required this.pubkey,
    required this.createdAt,
    required this.room,
    required this.root,
    required this.broadcast,
    required this.targets,
    required this.raw,
  });

  final String id;
  final int kind;
  final String pubkey;
  final int createdAt;
  final String? room;
  final String? root;
  final bool broadcast;
  final List<String> targets;
  final String raw;
}

/// The latest 39005 of a thread replaces the previous one.
class StoredSummary extends StoredEvent {
  const StoredSummary({
    required this.root,
    required this.room,
    required this.createdAt,
    required this.raw,
  });

  final String root;
  final String? room;
  final int createdAt;
  final String raw;
}

Iterable<List<dynamic>> _tags(Map<String, dynamic> event) {
  final tags = event['tags'];
  if (tags is! List) return const [];
  return tags.whereType<List<dynamic>>();
}

String? _value(List<dynamic> tag, int index) {
  if (tag.length <= index) return null;
  final value = tag[index];
  return value is String ? value : null;
}

String? _firstTag(Map<String, dynamic> event, String name) {
  for (final tag in _tags(event)) {
    if (_value(tag, 0) == name) return _value(tag, 1);
  }
  return null;
}

/// Same rule as `getThreadReference`: the last `reply`-marked `e` tag is the
/// parent, the `root`-marked one (else the parent) the root. No reply marker
/// means top-level.
String? _threadRoot(Map<String, dynamic> event) {
  final eTags = _tags(
    event,
  ).where((tag) => _value(tag, 0) == 'e' && _value(tag, 1) != null).toList();
  final parent = eTags.reversed
      .where((tag) => _value(tag, 3) == 'reply')
      .map((tag) => _value(tag, 1))
      .firstOrNull;
  if (parent == null) return null;
  return eTags
          .where((tag) => _value(tag, 3) == 'root')
          .map((tag) => _value(tag, 1))
          .firstOrNull ??
      parent;
}

/// Files [event] (relay JSON), or null when the store does not keep it.
StoredEvent? classifyStoredEvent(Map<String, dynamic> event) {
  final id = event['id'];
  final kind = event['kind'];
  final createdAt = event['created_at'];
  if (id is! String || kind is! int || createdAt is! int) return null;
  final room = _firstTag(event, 'h');
  if (kind == _threadSummary) {
    final root = _firstTag(event, 'e');
    if (root == null) return null;
    return StoredSummary(
      root: root,
      room: room,
      createdAt: createdAt,
      raw: jsonEncode(event),
    );
  }
  final aux = _auxKinds.contains(kind);
  if (!aux && !_contentKinds.contains(kind)) return null;
  // A reaction or deletion may name only its target; anything else without
  // a room cannot be placed.
  if (room == null && kind != 5 && kind != 7) return null;
  final pubkey = event['pubkey'];
  if (pubkey is! String) return null;
  return StoredRow(
    id: id,
    kind: kind,
    pubkey: pubkey,
    createdAt: createdAt,
    room: room,
    root: aux ? null : _threadRoot(event),
    broadcast: _tags(
      event,
    ).any((tag) => _value(tag, 0) == 'broadcast' && _value(tag, 1) == '1'),
    targets: aux
        ? [
            for (final tag in _tags(event))
              if (_value(tag, 0) == 'e' && _value(tag, 1) != null)
                _value(tag, 1)!,
          ]
        : const [],
    raw: jsonEncode(event),
  );
}
