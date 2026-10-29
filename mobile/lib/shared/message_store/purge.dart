/// Forgetting a room's stored copy: when the account's membership list drops
/// a room it had, when a room a list dropped comes back (a rejoin), and on
/// the relay's rejoin and `since_join` notices. Rules:
/// docs/message-sync.md#forgetting-a-room; cases:
/// test-fixtures/message-store-purge.json (shared with desktop `purge.rs`).
library;

import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:sqflite/sqflite.dart';

import 'head_coverage.dart' show forgetStoreHeads;
import 'stored_event.dart';

/// Per room: the `created_at` of the last rejoin notice handled
/// (`joined_at`), when the account left it (`left_at`, 0: not left; from a
/// relay notice, or the newest stored row when a list dropped it) and of the
/// last history notice handled (`history_at`). Plus the indexes a room's
/// purge deletes by.
const roomMarksSchema = [
  'CREATE TABLE IF NOT EXISTS room_purge('
      'room TEXT PRIMARY KEY, joined_at INTEGER NOT NULL DEFAULT 0, '
      'left_at INTEGER NOT NULL DEFAULT 0, '
      'history_at INTEGER NOT NULL DEFAULT 0)',
  'CREATE INDEX IF NOT EXISTS targets_id ON targets(id)',
  'CREATE INDEX IF NOT EXISTS summaries_room ON summaries(room)',
];

/// Rooms forgotten per transaction; between chunks other work runs, so a
/// long purge does not hold the writer.
const _roomsPerTransaction = 16;

/// A list may forget at most this many kept rooms, or half of them, at once;
/// beyond both it is taken for a bad answer and skipped.
const _maxSureLoss = 5;

/// What a relay system message (40099) about the store's account says.
sealed class RoomNotice {
  const RoomNotice();
}

/// `member_joined` naming the account. The relay sends it on every join,
/// the first one too.
class RoomJoined extends RoomNotice {
  const RoomJoined();
}

/// `member_left` by the account, or `member_removed` naming it.
class RoomLeft extends RoomNotice {
  const RoomLeft();
}

/// `history_changed`; [sinceJoin] when to `since_join`.
class RoomHistory extends RoomNotice {
  const RoomHistory(this.sinceJoin);

  final bool sinceJoin;
}

/// Whether [event] (relay JSON) is a notice for [account] signed by
/// [relayKey]: it then takes its own place in the write queue. The writer
/// checks the stored row again with [roomNotice].
bool mayBeRoomNotice(
  Map<String, dynamic> event,
  String relayKey,
  String account,
) =>
    event['kind'] == 40099 &&
    event['pubkey'] is String &&
    (event['pubkey'] as String).toLowerCase() == relayKey &&
    event['content'] is String &&
    _noticeIn(event['content'] as String, account) != null;

/// The notice [row] carries for [account], when it is a 40099 signed by
/// [relayKey] (the relay's NIP-11 `self`).
RoomNotice? roomNotice(StoredRow row, String account, String relayKey) {
  if (row.kind != 40099 ||
      row.room == null ||
      row.pubkey.toLowerCase() != relayKey.toLowerCase()) {
    return null;
  }
  try {
    final event = jsonDecode(row.raw);
    return _noticeIn((event as Map)['content'] as String, account);
  } catch (_) {
    return null;
  }
}

RoomNotice? _noticeIn(String json, String account) {
  try {
    final content = jsonDecode(json);
    if (content is! Map) return null;
    String? field(String name) =>
        content[name] is String ? content[name] as String : null;
    bool names(String name) =>
        field(name)?.toLowerCase() == account.toLowerCase();
    return switch (field('type')) {
      'member_joined' when names('target') => const RoomJoined(),
      'member_left'
          when field('target') == null ? names('actor') : names('target') =>
        const RoomLeft(),
      'member_removed' when names('target') => const RoomLeft(),
      'history_changed' when field('history') != null => RoomHistory(
        field('history') == 'since_join',
      ),
      _ => null,
    };
  } catch (_) {
    return null;
  }
}

/// The rooms of one file, kept in the writer's memory so a list is applied
/// without scanning the file. Loaded once when the writer opens it.
class StoredRooms {
  StoredRooms._(this.stored, this.left, this.kept);

  /// Rooms with stored rows, summaries or coverage (a superset is fine).
  final Set<String> stored;

  /// Rooms with a left mark.
  final Set<String> left;

  /// The last membership list applied (`null`: none yet).
  Set<String>? kept;

  /// A room was first stored since the last list was applied.
  bool fresh = false;

  static Future<StoredRooms> load(DatabaseExecutor db) async {
    Future<Set<String>> column(String sql) async => {
      for (final row in await db.rawQuery(sql))
        if (row.values.first case final String room) room,
    };
    Set<String>? kept;
    final rows = await db.rawQuery(
      "SELECT value FROM sync_state WHERE key = 'kept_rooms'",
    );
    try {
      if (rows.firstOrNull?['value'] case final String value) {
        kept = (jsonDecode(value) as List).whereType<String>().toSet();
      }
    } catch (_) {
      kept = null;
    }
    return StoredRooms._(
      await column(
        'SELECT room FROM events WHERE room IS NOT NULL '
        'UNION SELECT room FROM summaries WHERE room IS NOT NULL '
        'UNION SELECT room FROM coverage',
      ),
      await column('SELECT room FROM room_purge WHERE left_at > 0'),
      kept,
    );
  }

  /// Something of [room] is about to be written.
  void note(String room) {
    if (stored.add(room)) fresh = true;
  }
}

typedef _Marks = ({int joinedAt, int leftAt, int historyAt});

Future<_Marks> _marks(DatabaseExecutor db, String room) async {
  final rows = await db.rawQuery(
    'SELECT joined_at, left_at, history_at FROM room_purge WHERE room = ?',
    [room],
  );
  final row = rows.firstOrNull;
  int value(String column) => row?[column] is int ? row![column]! as int : 0;
  return (
    joinedAt: value('joined_at'),
    leftAt: value('left_at'),
    historyAt: value('history_at'),
  );
}

Future<void> _setMarks(DatabaseExecutor db, String room, _Marks marks) =>
    db.rawInsert(
      'INSERT OR REPLACE INTO room_purge(room, joined_at, left_at, history_at) '
      'VALUES (?, ?, ?, ?)',
      [room, marks.joinedAt, marks.leftAt, marks.historyAt],
    );

/// The largest `created_at` SQLite compares with: "every row".
const _everyRow = 0x7fffffffffffffff;

/// Deletes [room]'s rows created before [before] (every row when null), the
/// room-less overlays (reactions, deletions) aimed only at them (one also
/// naming a row that stays is kept, or that row would lose it), their thread
/// summaries (every summary of the room when null) and all the room's
/// coverage.
Future<void> _forgetRoom(DatabaseExecutor db, String room, int? before) async {
  final until = before ?? _everyRow;
  await db.execute(
    'CREATE TEMP TABLE IF NOT EXISTS purge_ids(id TEXT PRIMARY KEY)',
  );
  await db.rawDelete('DELETE FROM purge_ids');
  await db.rawInsert(
    'INSERT OR IGNORE INTO purge_ids SELECT id FROM events '
    'WHERE room = ? AND created_at < ?',
    [room, until],
  );
  await db.rawInsert(
    'INSERT OR IGNORE INTO purge_ids SELECT DISTINCT t.id FROM targets t '
    'JOIN events o ON o.id = t.id WHERE o.room IS NULL AND t.target IN '
    '(SELECT id FROM events WHERE room = ?1 AND created_at < ?2) '
    'AND NOT EXISTS (SELECT 1 FROM targets u WHERE u.id = t.id AND '
    'u.target NOT IN (SELECT id FROM events WHERE room = ?1 AND created_at < ?2))',
    [room, until],
  );
  await db.rawDelete(
    'DELETE FROM summaries WHERE (room = ? AND ?) '
    'OR root IN (SELECT id FROM purge_ids)',
    [room, before == null ? 1 : 0],
  );
  await db.rawDelete(
    'DELETE FROM targets WHERE id IN (SELECT id FROM purge_ids)',
  );
  await db.rawDelete(
    'DELETE FROM events WHERE id IN (SELECT id FROM purge_ids)',
  );
  await db.rawDelete('DELETE FROM purge_ids');
  await db.rawDelete('DELETE FROM coverage WHERE room = ?', [room]);
}

/// Forgets a room a list dropped and marks it left as of its newest stored
/// row (no mark when it had none): a rejoin notice older than that is from
/// an earlier membership. Returns whether it was marked.
Future<bool> _forgetDropped(DatabaseExecutor db, String room) async {
  final rows = await db.rawQuery(
    'SELECT max(coalesce((SELECT max(created_at) FROM events WHERE room = ?1), 0), '
    'coalesce((SELECT max(created_at) FROM summaries WHERE room = ?1), 0)) AS newest',
    [room],
  );
  final newest = rows.first['newest'] as int? ?? 0;
  await _forgetRoom(db, room, null);
  if (newest <= 0) return false;
  final marks = await _marks(db, room);
  await _setMarks(db, room, (
    joinedAt: marks.joinedAt,
    leftAt: marks.leftAt > newest ? marks.leftAt : newest,
    historyAt: marks.historyAt,
  ));
  return true;
}

/// Applies the account's membership list [keep]
/// (docs/message-sync.md#forgetting-a-room):
/// - the first list (none kept yet) is only recorded;
/// - a room of the last list that [keep] lacks is forgotten and marked left;
/// - a room of [keep] marked left (a list dropped it) is forgotten again,
///   its rows are from while the account was out, and loses the mark;
/// - rooms no list had (open channels only browsed) are left alone;
/// - rooms [skip] names (first written after the list's fetch began) are
///   left alone, a dropped one stays kept for the next list;
/// - a list dropping more than [_maxSureLoss] rooms and half the kept ones
///   is skipped, as is one equal to the last with no room stored since
///   and none of it marked left.
///
/// Runs its own transactions, a chunk of rooms each. Returns the rooms
/// forgotten.
Future<List<String>> applyList(
  Database db,
  StoredRooms rooms,
  Set<String> keep,
  bool Function(String room) skip,
) async {
  final previous = rooms.kept;
  final unchanged =
      previous != null &&
      setEquals(previous, keep) &&
      !rooms.fresh &&
      !keep.any(rooms.left.contains);
  if (keep.isEmpty || unchanged) {
    return const [];
  }
  final kept = Set.of(keep);
  final doomed = <(String, bool)>[];
  if (previous != null) {
    final dropped = previous.difference(keep);
    if (dropped.length > _maxSureLoss && dropped.length * 2 > previous.length) {
      debugPrint(
        '[MessageStore] a list dropping ${dropped.length} of '
        '${previous.length} rooms was not applied',
      );
      return const [];
    }
    for (final room in dropped) {
      if (skip(room)) {
        kept.add(room);
      } else if (rooms.stored.contains(room)) {
        doomed.add((room, false));
      }
    }
    for (final room in keep.intersection(rooms.left)) {
      if (rooms.stored.contains(room) && !skip(room)) doomed.add((room, true));
    }
  }
  final back = keep.intersection(rooms.left);
  final forgotten = <String>[];
  for (var start = 0; start < doomed.length; start += _roomsPerTransaction) {
    if (start > 0) await Future<void>.delayed(Duration.zero);
    final chunk = doomed.skip(start).take(_roomsPerTransaction);
    await db.transaction((txn) async {
      for (final (room, rejoined) in chunk) {
        if (rejoined) {
          await _forgetRoom(txn, room, null);
        } else if (await _forgetDropped(txn, room)) {
          rooms.left.add(room);
        }
        rooms.stored.remove(room);
        forgotten.add(room);
      }
    });
  }
  await db.transaction((txn) async {
    for (final room in back) {
      final marks = await _marks(txn, room);
      await _setMarks(txn, room, (
        joinedAt: marks.joinedAt,
        leftAt: 0,
        historyAt: marks.historyAt,
      ));
      rooms.left.remove(room);
    }
    await txn.rawInsert(
      "INSERT OR REPLACE INTO sync_state(key, value) VALUES ('kept_rooms', ?)",
      [jsonEncode(kept.toList()..sort())],
    );
  });
  rooms
    ..kept = kept
    ..fresh = false;
  return forgotten;
}

/// Acts on a relay notice before it is written. Returns whether the room
/// was forgotten.
/// - [RoomJoined] newer than the last one handled is recorded; only when
///   the room is marked left before it does it forget the rows older than
///   it (and clear the mark): the first join keeps a room's history.
/// - [RoomLeft] newer than the last join marks the room left.
/// - [RoomHistory] newer than the last one handled is recorded; only
///   `since_join` forgets the room's rows older than it: newer ones were
///   sent to this account under the new rule.
Future<bool> applyRoomNotice(
  DatabaseExecutor db,
  StoredRooms rooms,
  StoredRow row,
  RoomNotice notice,
) async {
  final room = row.room;
  if (room == null) return false;
  final at = row.createdAt;
  final marks = await _marks(db, room);
  // Any history change may show rows a head span said were not there
  // (docs/message-sync.md#head-spans).
  if (notice is RoomHistory) await forgetStoreHeads(db, room);
  switch (notice) {
    case RoomJoined() when at > marks.joinedAt:
      final rejoined = marks.leftAt > 0 && at > marks.leftAt;
      if (rejoined) {
        await _forgetRoom(db, room, at);
        rooms.left.remove(room);
      }
      await _setMarks(db, room, (
        joinedAt: at,
        leftAt: rejoined ? 0 : marks.leftAt,
        historyAt: marks.historyAt,
      ));
      return rejoined;
    case RoomLeft() when at > marks.joinedAt && at > marks.leftAt:
      rooms.left.add(room);
      await _setMarks(db, room, (
        joinedAt: marks.joinedAt,
        leftAt: at,
        historyAt: marks.historyAt,
      ));
      return false;
    case RoomHistory(:final sinceJoin) when at > marks.historyAt:
      if (sinceJoin) await _forgetRoom(db, room, at);
      await _setMarks(db, room, (
        joinedAt: marks.joinedAt,
        leftAt: marks.leftAt,
        historyAt: at,
      ));
      return sinceJoin;
    default:
      return false;
  }
}
