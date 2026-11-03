part of 'message_store.dart';

/// How a [MessageStore.submitPushed] write ended
/// (docs/message-sync.md#push-inbox). [written] only once its transaction
/// committed; [accepted] are the events the store kept (a room the account
/// is in, not forgotten since the fetch), the only ones to fold.
@immutable
class PushedWriteOutcome {
  const PushedWriteOutcome._(this.written, this.accepted);

  const PushedWriteOutcome.written(Set<String> accepted)
    : this._(true, accepted);

  /// Not written: the queue was full or cleared, the write failed, or the
  /// lease ended. The inbox files stay for the next drain.
  static const dropped = PushedWriteOutcome._(false, <String>{});

  final bool written;
  final Set<String> accepted;

  @override
  String toString() => written
      ? 'PushedWriteOutcome.written(${accepted.length})'
      : 'PushedWriteOutcome.dropped';
}

/// A room forgotten this close before a pushed fetch (or after it) gets
/// none of its rows: same-device clock, seconds.
const _pushedForgotMargin = 5;

/// Events the push inbox holds, fetched at [fetchedAt] (unix seconds).
class _PushedOp {
  _PushedOp(this.events, this.fetchedAt);

  final List<Map<String, dynamic>> events;
  final int fetchedAt;
  final Completer<PushedWriteOutcome> done = Completer();

  void complete(PushedWriteOutcome outcome) {
    if (!done.isCompleted) done.complete(outcome);
  }

  void drop() => complete(PushedWriteOutcome.dropped);
}

extension _Pushed on MessageStore {
  /// Writes [op]'s room rows that the account may see now: rooms of the last
  /// membership list, not marked left, not forgotten since
  /// [_PushedOp.fetchedAt] (less [_pushedForgotMargin]). They set no
  /// coverage, no head mark and no `born`: a list fetched meanwhile still
  /// drops their room. Each is marked pushed until the relay's copy arrives.
  Future<void> _writePushed(_Writer writer, _PushedOp op) async {
    try {
      final rows = [
        for (final event in op.events)
          if (classifyStoredEvent(event) case final StoredRow row
              when row.room != null)
            row,
      ];
      if (rows.isEmpty) {
        op.complete(const PushedWriteOutcome.written({}));
        return;
      }
      final db = await (writer.database ??= writer.open());
      if (writer.stopped) return op.drop();
      final known = writer.rooms ??= await purge.StoredRooms.load(db);
      final pushed = writer.pushedIds ??= await _loadPushedIds(db);
      final accepted = <String>{};
      await db.transaction((txn) async {
        final rooms = {for (final row in rows) row.room!}.toList();
        final forgot = {
          for (final mark in await txn.rawQuery(
            'SELECT room, at FROM room_forgot '
            'WHERE room IN (${List.filled(rooms.length, '?').join(', ')})',
            rooms,
          ))
            mark['room']! as String: mark['at']! as int,
        };
        final kept = known.kept;
        bool allowed(String room) =>
            kept != null &&
            kept.contains(room) &&
            !known.left.contains(room) &&
            (forgot[room] == null ||
                forgot[room]! < op.fetchedAt - _pushedForgotMargin);
        final batch = txn.batch();
        for (final row in rows) {
          final room = row.room!;
          if (!allowed(room)) continue;
          // Before the row: a relay copy already stored is not marked.
          batch.rawInsert(
            'INSERT OR IGNORE INTO pushed_rows(id) SELECT ? '
            'WHERE NOT EXISTS (SELECT 1 FROM events WHERE id = ?)',
            [row.id, row.id],
          );
          known.note(room);
          _write(batch, row);
          accepted.add(row.id);
        }
        await batch.commit(noResult: true);
      });
      if (writer.stopped) return op.drop();
      pushed.addAll(accepted);
      op.complete(PushedWriteOutcome.written(accepted));
    } catch (_) {
      op.drop();
      rethrow;
    }
  }
}

Future<Set<String>> _loadPushedIds(DatabaseExecutor db) async => {
  for (final row in await db.rawQuery('SELECT id FROM pushed_rows'))
    row['id']! as String,
};
