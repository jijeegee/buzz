part of 'message_store.dart';

/// The freshness token of each room's coverage head
/// (docs/message-sync.md#watermark-room_seq).
const roomSeqSchema =
    'CREATE TABLE IF NOT EXISTS room_seq('
    'room TEXT PRIMARY KEY, seq INTEGER NOT NULL, at INTEGER NOT NULL)';

/// The `sync_state` key of the last stream epoch.
const userStreamEpochKey = 'user_stream_epoch';

/// The largest seq a snapshot may carry (2^53 - 1).
const _maxRoomSeq = 9007199254740991;

/// A `room_seq` change of its own: an accepted part 1's [epoch] (clears every
/// row when it differs from the stored one), or [ahead] rows to delete
/// (stored seq above the snapshot's after a relay restore).
class _StreamSeqOp {
  const _StreamSeqOp({this.epoch, this.ahead});

  final String? epoch;
  final Map<String, int>? ahead;
}

Future<String?> _storedEpoch(DatabaseExecutor db) async {
  final rows = await db.rawQuery('SELECT value FROM sync_state WHERE key = ?', [
    userStreamEpochKey,
  ]);
  return rows.firstOrNull?['value'] as String?;
}

/// Rule W's write (docs/message-sync.md#watermark-room_seq): only under the
/// epoch the walk started in, never lowering a stored seq; an equal seq
/// moves `at`. Runs in the span's own transaction, for its kept rooms.
Future<void> _writeRoomSeqs(
  Transaction txn,
  List<String> rooms,
  _CoverageOp coverage,
) async {
  final seqs = coverage.seqs;
  final epoch = coverage.epoch;
  if (seqs == null || epoch == null || rooms.isEmpty) return;
  if (await _storedEpoch(txn) != epoch) return;
  for (final room in rooms) {
    final seq = seqs[room];
    if (seq == null || seq < 0 || seq > _maxRoomSeq) continue;
    final rows = await txn.rawQuery('SELECT seq FROM room_seq WHERE room = ?', [
      room,
    ]);
    final stored = rows.firstOrNull?['seq'] as int?;
    if (stored != null && seq < stored) continue;
    await txn.rawInsert(
      'INSERT OR REPLACE INTO room_seq(room, seq, at) VALUES (?, ?, ?)',
      [room, seq, coverage.to],
    );
  }
}

/// The user stream's state in the file's `sync_state`
/// (docs/message-sync.md#counters).
extension MessageStoreUserStream on MessageStore {
  Future<void> _applyStreamSeqOp(_Writer writer, _StreamSeqOp op) async {
    final db = await (writer.database ??= writer.open());
    if (writer.stopped) return;
    await db.transaction((txn) async {
      if (op.epoch case final epoch?) {
        if (await _storedEpoch(txn) != epoch) {
          await txn.rawDelete('DELETE FROM room_seq');
          await txn.rawInsert(
            'INSERT OR REPLACE INTO sync_state(key, value) VALUES (?, ?)',
            [userStreamEpochKey, epoch],
          );
        }
      }
      for (final MapEntry(key: room, value: seq)
          in (op.ahead ?? const <String, int>{}).entries) {
        await txn.rawDelete('DELETE FROM room_seq WHERE room = ? AND seq > ?', [
          room,
          seq,
        ]);
      }
    });
  }

  /// An accepted part 1's [epoch]: queued before any rule-W write of its
  /// connection; clears every row when the stored epoch differs.
  void queueStreamEpoch(MessageStoreLease? lease, String epoch) =>
      _queueStreamSeq(lease, _StreamSeqOp(epoch: epoch));

  /// Deletes the rows of [ahead] whose stored seq is above the snapshot's.
  void deleteAheadRoomSeqs(MessageStoreLease? lease, Map<String, int> ahead) {
    if (ahead.isNotEmpty) {
      _queueStreamSeq(lease, _StreamSeqOp(ahead: Map.of(ahead)));
    }
  }

  void _queueStreamSeq(MessageStoreLease? lease, _StreamSeqOp op) {
    final writer = _writerFor(lease);
    if (writer == null) return;
    writer.enqueue(op);
    _schedule(writer);
  }

  /// Waits (at most [cap]) for the writes queued so far; false on timeout or
  /// when the lease has ended.
  Future<bool> _settled(MessageStoreLease? lease, Duration cap) async {
    final writer = _writerFor(lease);
    if (writer == null) return false;
    try {
      await writer.barrier().timeout(cap);
    } on TimeoutException {
      return false;
    }
    return !writer.stopped;
  }

  /// `room -> (seq, at)` of [rooms], after the writes queued so far.
  Future<Map<String, (int, int)>> roomSeqs(
    MessageStoreLease? lease,
    Iterable<String> rooms, {
    Duration cap = const Duration(seconds: 10),
  }) async {
    final wanted = rooms.toList();
    if (wanted.isEmpty || !await _settled(lease, cap)) return const {};
    final rows = await _read(lease, 'room seqs', (db) async {
      final found = <String, (int, int)>{};
      for (final room in wanted) {
        final rows = await db.rawQuery(
          'SELECT seq, at FROM room_seq WHERE room = ?',
          [room],
        );
        if (rows.firstOrNull case {'seq': final int seq, 'at': final int at}) {
          found[room] = (seq, at);
        }
      }
      return found;
    });
    return rows ?? const {};
  }

  /// The stored epoch, after the writes queued so far.
  Future<String?> streamEpoch(MessageStoreLease? lease) async {
    if (!await _settled(lease, const Duration(seconds: 10))) return null;
    return _read<String?>(lease, 'stream epoch', _storedEpoch);
  }

  /// Of [ids], those already stored, after the writes queued so far; `null`
  /// when that cannot be told.
  Future<Set<String>?> storedEventIds(
    MessageStoreLease? lease,
    Iterable<String> ids, {
    Duration cap = const Duration(seconds: 10),
  }) async {
    final wanted = ids.toSet().toList();
    if (!await _settled(lease, cap)) return null;
    if (wanted.isEmpty) return const {};
    return _read(lease, 'stored ids', (db) async {
      final found = <String>{};
      for (var i = 0; i < wanted.length; i += 500) {
        final part = wanted.sublist(i, math.min(i + 500, wanted.length));
        final rows = await db.rawQuery(
          'SELECT id FROM events WHERE id IN '
          '(${List.filled(part.length, '?').join(', ')})',
          part,
        );
        for (final row in rows) {
          if (row['id'] case final String id) found.add(id);
        }
      }
      return found;
    }).then((found) => found ?? <String>{});
  }

  /// Each room's newest span begun before [before]: `(lo, hi)`.
  Future<Map<String, (int, int)>> coverageHeads(
    MessageStoreLease? lease,
    Iterable<String> rooms,
    int before,
  ) async {
    final heads = await _read(
      lease,
      'coverage heads',
      (db) => spans.coverageHeads(db, rooms, before),
    );
    return heads ?? const {};
  }

  /// The saved values of [keys] in [lease]'s file; empty when there is no
  /// file yet or the lease has ended.
  Future<Map<String, String>> readSyncState(
    MessageStoreLease? lease,
    List<String> keys,
  ) async {
    if (keys.isEmpty) return const {};
    final values = await _read(lease, 'sync state', (db) async {
      final rows = await db.rawQuery(
        'SELECT key, value FROM sync_state WHERE key IN '
        '(${List.filled(keys.length, '?').join(', ')})',
        keys,
      );
      return {
        for (final row in rows)
          if (row['key'] case final String key)
            if (row['value'] case final String value) key: value,
      };
    });
    return values ?? const {};
  }

  /// Saves [values] in [lease]'s file after the writes queued before it;
  /// nothing once the lease has ended.
  Future<void> writeSyncState(
    MessageStoreLease? lease,
    Map<String, String> values,
  ) {
    final writer = _writerFor(lease);
    if (writer == null || values.isEmpty) return Future.value();
    final saved = writer.drained.then((_) async {
      if (writer.stopped) return;
      try {
        final db = await (writer.database ??= writer.open());
        if (writer.stopped) return;
        final batch = db.batch();
        for (final MapEntry(:key, :value) in values.entries) {
          batch.rawInsert(
            'INSERT OR REPLACE INTO sync_state(key, value) VALUES (?, ?)',
            [key, value],
          );
        }
        await batch.commit(noResult: true);
      } catch (error) {
        debugPrint('[MessageStore] sync state write failed: $error');
      }
    });
    writer.drained = saved;
    return saved;
  }
}
