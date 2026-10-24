import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:sqlite3/sqlite3.dart';

import '../relay/relay.dart';
import 'coverage.dart';
import 'event_rows.dart';

/// The local message database: every message this device receives, stored
/// once as the relay's JSON, plus what is known to be complete. One file per
/// server scope and account, so no table needs a scope column and removing an
/// account removes its file.
///
/// Schema (see RESEARCH/MOBILE_MESSAGE_DB_DESIGN_20261024.md):
/// - `events`: one row per event. `stream` is `''` for a main-timeline row,
///   the root id for a thread reply, null for overlays. A deleted event keeps
///   its row (so it cannot come back) with its content blanked.
/// - `deletions`: every deleted id, so a target that arrives later is stored
///   already deleted.
/// - `coverage`, `room_state`, `sync_state`: see coverage.dart.
abstract final class MessageDbSchema {
  /// Stored in `PRAGMA user_version`. A file with any other version is moved
  /// aside and replaced by an empty one.
  static const version = 1;

  static const _create = [
    'CREATE TABLE events ('
        'id TEXT PRIMARY KEY, '
        'channel_id TEXT NOT NULL, '
        'kind INTEGER NOT NULL, '
        'pubkey TEXT NOT NULL, '
        'created_at INTEGER NOT NULL, '
        'received_at INTEGER NOT NULL, '
        'stream TEXT, '
        'target_id TEXT, '
        'deleted INTEGER NOT NULL DEFAULT 0, '
        'raw TEXT NOT NULL)',
    // Newest first in the relay's order (ascending id within a second).
    'CREATE INDEX ev_stream ON events '
        '(channel_id, stream, created_at DESC, id) '
        'WHERE stream IS NOT NULL AND deleted = 0',
    'CREATE INDEX ev_target ON events (target_id) WHERE target_id IS NOT NULL',
    // One thread summary per root: the newest.
    'CREATE UNIQUE INDEX ev_summary ON events (target_id) '
        'WHERE kind = ${EventKind.channelThreadSummary}',
    'CREATE TABLE deletions ('
        'target_id TEXT PRIMARY KEY, '
        'deletion_id TEXT NOT NULL, '
        'created_at INTEGER NOT NULL) WITHOUT ROWID',
    'CREATE TABLE coverage ('
        'channel_id TEXT NOT NULL, '
        'stream TEXT NOT NULL, '
        'lo_at INTEGER NOT NULL, lo_id TEXT NOT NULL, '
        'hi_at INTEGER NOT NULL, hi_id TEXT NOT NULL, '
        'PRIMARY KEY (channel_id, stream, lo_at, lo_id)) WITHOUT ROWID',
    'CREATE TABLE room_state ('
        'channel_id TEXT PRIMARY KEY, '
        'live_from INTEGER) WITHOUT ROWID',
    'CREATE TABLE sync_state ('
        'origin TEXT PRIMARY KEY, '
        'watermark INTEGER, '
        'live_since INTEGER) WITHOUT ROWID',
  ];
}

/// Opens (creating when needed) the read-write connection at [path]. Only
/// the writer isolate holds one.
Database openMessageDb(String path) {
  File(path).parent.createSync(recursive: true);
  var db = _openOrMoveAside(path);
  final version = db.select('PRAGMA user_version').first.values.first as int;
  if (version != 0 && version != MessageDbSchema.version) {
    // Written by a newer (or unknown) build: never guess at its layout.
    db.close();
    _moveAside(path, 'v$version');
    db = sqlite3.open(path);
  }
  try {
    _configure(db);
    _migrate(db);
  } catch (_) {
    db.close();
    rethrow;
  }
  return db;
}

/// A connection for reads on the UI isolate: never writes, and gives up
/// quickly rather than block a frame.
Database openMessageDbReader(String path) {
  final db = sqlite3.open(path, mode: OpenMode.readOnly);
  db.execute('PRAGMA query_only = 1');
  db.execute('PRAGMA busy_timeout = 50');
  return db;
}

Database _openOrMoveAside(String path) {
  Database? db;
  try {
    db = sqlite3.open(path);
    // Fails on a file that is not a database.
    db.select('PRAGMA schema_version');
    return db;
  } on SqliteException {
    db?.close();
    _moveAside(path, 'corrupt');
    return sqlite3.open(path);
  }
}

/// Renames the database and its side files to `<path>.<reason>.<time>`, kept
/// for diagnosis; the next open starts empty.
void _moveAside(String path, String reason) {
  final stamp = DateTime.now().millisecondsSinceEpoch;
  for (final suffix in const ['', '-wal', '-shm', '-journal']) {
    final file = File('$path$suffix');
    if (file.existsSync()) file.renameSync('$path.$reason.$stamp$suffix');
  }
  debugPrint('[MessageDb] moved $path aside ($reason)');
}

void _configure(Database db) {
  // Must precede the first table; a later deletion stage frees pages with
  // `PRAGMA incremental_vacuum`.
  db.execute('PRAGMA auto_vacuum = INCREMENTAL');
  db.execute('PRAGMA journal_mode = WAL');
  db.execute('PRAGMA synchronous = NORMAL');
  db.execute('PRAGMA busy_timeout = 5000');
}

void _migrate(Database db) {
  final version = db.select('PRAGMA user_version').first.values.first as int;
  if (version == MessageDbSchema.version) return;
  db.execute('BEGIN IMMEDIATE');
  try {
    for (final statement in MessageDbSchema._create) {
      db.execute(statement);
    }
    db.execute('PRAGMA user_version = ${MessageDbSchema.version}');
    db.execute('COMMIT');
  } catch (_) {
    db.execute('ROLLBACK');
    rethrow;
  }
}

/// One transaction's worth of changes.
@immutable
class MessageDbWrite {
  const MessageDbWrite({
    this.rows = const [],
    this.claims = const [],
    this.liveFrom = const {},
    this.sync,
  });

  final List<MessageRow> rows;

  /// Coverage the fetch that returned [rows] proves (see coverage.dart).
  final List<CoverageClaim> claims;

  /// Room live ranges to start (a time) or end (null); for catch-up.
  final Map<String, int?> liveFrom;
  final SyncStateUpdate? sync;

  bool get isEmpty =>
      rows.isEmpty && claims.isEmpty && liveFrom.isEmpty && sync == null;
}

/// Applies [write] in one transaction: all of it lands or none of it does,
/// so a watermark can never move past events that were not stored.
void applyMessageWrite(Database db, MessageDbWrite write) {
  if (write.isEmpty) return;
  db.execute('BEGIN IMMEDIATE');
  try {
    final writer = _RowWriter(db);
    try {
      for (final row in write.rows) {
        writer.write(row);
      }
    } finally {
      writer.dispose();
    }
    for (final claim in write.claims) {
      applyCoverageClaim(db, claim);
    }
    applyLiveRanges(db, write.liveFrom);
    if (write.sync case final sync?) applySyncState(db, sync);
    db.execute('COMMIT');
  } catch (_) {
    if (!db.autocommit) db.execute('ROLLBACK');
    rethrow;
  }
}

/// What a deleted event's `raw` becomes: its identity and room, no content.
String deletedEventStub(MessageRow row) => jsonEncode({
  'id': row.id,
  'pubkey': row.event.pubkey,
  'created_at': row.event.createdAt,
  'kind': row.kind,
  'tags': [
    ['h', row.channelId],
  ],
  'content': '',
});

const _stubSql =
    "json_object('id', id, 'pubkey', pubkey, 'created_at', created_at, "
    "'kind', kind, 'tags', json_array(json_array('h', channel_id)), "
    "'content', '')";

class _RowWriter {
  _RowWriter(Database db)
    : _insert = db.prepare(
        'INSERT INTO events (id, channel_id, kind, pubkey, created_at, '
        'received_at, stream, target_id, deleted, raw) '
        'VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) '
        'ON CONFLICT (id) DO NOTHING',
      ),
      _upsertSummary = db.prepare(
        'INSERT INTO events (id, channel_id, kind, pubkey, created_at, '
        'received_at, stream, target_id, deleted, raw) '
        'VALUES (?, ?, ?, ?, ?, ?, NULL, ?, 0, ?) '
        'ON CONFLICT (id) DO NOTHING '
        'ON CONFLICT (target_id) WHERE kind = '
        '${EventKind.channelThreadSummary} DO UPDATE SET '
        'id = excluded.id, channel_id = excluded.channel_id, '
        'pubkey = excluded.pubkey, created_at = excluded.created_at, '
        'received_at = excluded.received_at, raw = excluded.raw '
        // The same second keeps arrival order: the later one wins.
        'WHERE excluded.created_at >= events.created_at',
      ),
      _isDeleted = db.prepare('SELECT 1 FROM deletions WHERE target_id = ?'),
      _recordDeletion = db.prepare(
        'INSERT INTO deletions (target_id, deletion_id, created_at) '
        'VALUES (?, ?, ?) ON CONFLICT (target_id) DO NOTHING',
      ),
      _blankTarget = db.prepare(
        'UPDATE events SET deleted = 1, raw = $_stubSql '
        'WHERE id = ? AND deleted = 0',
      ),
      // A deleted message's edits hold its old text; blank them as well.
      _blankEdits = db.prepare(
        'UPDATE events SET deleted = 1, raw = $_stubSql '
        'WHERE target_id = ? AND kind = ${EventKind.streamMessageEdit} '
        'AND deleted = 0',
      );

  final PreparedStatement _insert;
  final PreparedStatement _upsertSummary;
  final PreparedStatement _isDeleted;
  final PreparedStatement _recordDeletion;
  final PreparedStatement _blankTarget;
  final PreparedStatement _blankEdits;

  void write(MessageRow row) {
    if (row.isDeletion) {
      for (final target in row.deleteTargets) {
        _recordDeletion.execute([target, row.id, row.event.createdAt]);
        _blankTarget.execute([target]);
        _blankEdits.execute([target]);
      }
      return;
    }
    final event = row.event;
    if (row.isSummary) {
      _upsertSummary.execute([
        row.id,
        row.channelId,
        row.kind,
        event.pubkey,
        event.createdAt,
        row.receivedAt,
        row.targetId,
        row.encodeRaw(),
      ]);
      return;
    }
    final deleted =
        _deleted(row.id) ||
        (row.kind == EventKind.streamMessageEdit &&
            row.targetId != null &&
            _deleted(row.targetId!));
    _insert.execute([
      row.id,
      row.channelId,
      row.kind,
      event.pubkey,
      event.createdAt,
      row.receivedAt,
      row.stream,
      row.targetId,
      deleted ? 1 : 0,
      deleted ? deletedEventStub(row) : row.encodeRaw(),
    ]);
  }

  bool _deleted(String id) => _isDeleted.select([id]).isNotEmpty;

  void dispose() {
    _insert.close();
    _upsertSummary.close();
    _isDeleted.close();
    _recordDeletion.close();
    _blankTarget.close();
    _blankEdits.close();
  }
}
