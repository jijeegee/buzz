/// Small saved values drawn at launch before the relay answers (channel
/// list, profiles), kept in the account's message store file so they go
/// with it at sign-out and no other account's file holds them
/// (docs/message-sync.md#launch-order).
library;

import 'package:sqflite/sqflite.dart';

const snapshotSchema =
    'CREATE TABLE IF NOT EXISTS snapshot(key TEXT PRIMARY KEY, '
    'value TEXT NOT NULL, updated_at INTEGER NOT NULL)';

Future<String?> readSnapshotValue(DatabaseExecutor db, String key) async {
  final rows = await db.rawQuery('SELECT value FROM snapshot WHERE key = ?', [
    key,
  ]);
  final value = rows.firstOrNull?['value'];
  return value is String ? value : null;
}

/// Values whose key starts with [prefix], newest first, at most [limit].
Future<Map<String, String>> readSnapshotValues(
  DatabaseExecutor db,
  String prefix,
  int limit,
) async {
  final rows = await db.rawQuery(
    'SELECT key, value FROM snapshot WHERE key >= ? AND key < ? '
    'ORDER BY updated_at DESC LIMIT ?',
    [prefix, _prefixEnd(prefix), limit],
  );
  return {
    for (final row in rows)
      if (row['key'] case final String key)
        if (row['value'] case final String value) key: value,
  };
}

/// Writes [values] in one transaction; then, when [trimPrefix] is given,
/// keeps only the [keep] newest keys under it.
Future<void> writeSnapshotValues(
  Database db,
  Map<String, String> values, {
  String? trimPrefix,
  int keep = 0,
}) => db.transaction((txn) async {
  final now = DateTime.now().microsecondsSinceEpoch;
  final batch = txn.batch();
  var order = 0;
  for (final MapEntry(:key, :value) in values.entries) {
    batch.rawInsert(
      'INSERT OR REPLACE INTO snapshot(key, value, updated_at) VALUES (?, ?, ?)',
      [key, value, now + order++],
    );
  }
  await batch.commit(noResult: true);
  if (trimPrefix == null) return;
  await txn.rawDelete(
    'DELETE FROM snapshot WHERE key >= ? AND key < ? AND key NOT IN '
    '(SELECT key FROM snapshot WHERE key >= ? AND key < ? '
    'ORDER BY updated_at DESC LIMIT ?)',
    [
      trimPrefix,
      _prefixEnd(trimPrefix),
      trimPrefix,
      _prefixEnd(trimPrefix),
      keep,
    ],
  );
});

/// The first key past every key starting with [prefix].
String _prefixEnd(String prefix) =>
    prefix.substring(0, prefix.length - 1) +
    String.fromCharCode(prefix.codeUnitAt(prefix.length - 1) + 1);
