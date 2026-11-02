part of 'message_store.dart';

/// The user stream's state in the file's `sync_state`
/// (docs/message-sync.md#counters).
extension MessageStoreUserStream on MessageStore {
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
