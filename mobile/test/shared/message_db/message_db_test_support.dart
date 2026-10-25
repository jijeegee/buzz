import 'dart:convert';

import 'package:kuzz/shared/message_db/event_rows.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:sqlite3/sqlite3.dart';

const testChannel = 'room-1';

NostrEvent testEvent(
  String id, {
  int kind = EventKind.streamMessage,
  int createdAt = 100,
  String channel = testChannel,
  List<List<String>> tags = const [],
  String content = 'hello',
  String pubkey = 'author',
}) => NostrEvent(
  id: id,
  pubkey: pubkey,
  createdAt: createdAt,
  kind: kind,
  tags: [
    ['h', channel],
    ...tags,
  ],
  content: content,
);

NostrEvent testReply(String id, String rootId, {int createdAt = 100}) =>
    testEvent(
      id,
      createdAt: createdAt,
      tags: [
        ['e', rootId, '', 'root'],
        ['e', rootId, '', 'reply'],
      ],
    );

NostrEvent testOverlay(
  String id,
  int kind,
  String target, {
  int createdAt = 100,
  String content = '',
}) => testEvent(
  id,
  kind: kind,
  createdAt: createdAt,
  content: content,
  tags: [
    ['e', target],
  ],
);

List<MessageRow> rowsOf(Iterable<NostrEvent> events) =>
    messageRowsFor(events, receivedAt: 1);

/// The stored row of [id]: (deleted, content), or null when absent.
({bool deleted, String content, String id})? storedRow(Database db, String id) {
  final rows = db.select('SELECT deleted, raw FROM events WHERE id = ?', [id]);
  if (rows.isEmpty) return null;
  final raw = jsonDecode(rows.single['raw'] as String) as Map<String, dynamic>;
  return (
    deleted: rows.single['deleted'] == 1,
    content: raw['content'] as String,
    id: raw['id'] as String,
  );
}

List<String> storedIds(Database db) => [
  for (final row in db.select('SELECT id FROM events ORDER BY id'))
    row['id'] as String,
];
