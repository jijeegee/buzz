part of 'message_store.dart';

/// Replace the stored rows of a fetched page's key range, with the ids of
/// its room's rows written since the mark [captured] (from
/// [MessageStore.headSent]) kept.
class _WindowReplaceOp {
  const _WindowReplaceOp(this.page, this.captured);

  final StoreWindowPage page;
  final int captured;
}

/// A catch-up skipped or gave up on [rooms].
class _WindowRefetchOp {
  const _WindowRefetchOp(this.rooms);

  final List<String> rooms;
}

/// Checked windows (lib/shared/relay/window_check.dart).
extension MessageStoreWindows on MessageStore {
  /// [window]'s rows as the store holds them for a check, once everything
  /// received so far is written; `null` when that cannot be known (loss,
  /// the writer behind) or the read fails.
  Future<StoredCheckWindow?> checkWindow(
    MessageStoreLease? lease,
    StoreWindowKey window, {
    required int limit,
  }) async {
    if (!await _drainedFor(lease)) return null;
    return _read(
      lease,
      'window check',
      (db) =>
          db.transaction((txn) => readCheckWindow(txn, window, limit: limit)),
    );
  }

  /// Queues the replacement of [page]'s key range, asked under [capture]
  /// (a mark from [headSent] taken before the page went out). Refused
  /// (false) when its scope is not [current] or has ended.
  bool replaceWindow(
    StoreCapture? capture,
    MessageStoreLease? current,
    StoreWindowPage page,
  ) {
    if (capture == null || capture.lease != current) return false;
    final writer = _writerFor(capture.lease);
    if (writer == null) return false;
    writer.enqueue(_WindowReplaceOp(page, capture.sequence));
    _schedule(writer);
    return true;
  }

  /// Marks [rooms]' stored windows as needing their pages again.
  void markWindowsRefetch(MessageStoreLease? lease, Iterable<String> rooms) {
    final list = rooms.toSet().toList();
    final writer = _writerFor(lease);
    if (writer == null || list.isEmpty) return;
    writer.enqueue(_WindowRefetchOp(list));
    _schedule(writer);
  }

  Future<void> _applyWindowOp(_Writer writer, Object op) async {
    final db = await (writer.database ??= writer.open());
    if (writer.stopped) return;
    switch (op) {
      case _WindowRefetchOp(:final rooms):
        await writeWindowsRefetch(db, rooms);
      case _WindowReplaceOp(:final page, :final captured):
        final closed = writer.marks.close(captured);
        final room = page.window.room;
        // As for a head span: never for a room forgotten or changed after
        // the request went out.
        if (closed == null ||
            !closed.rooms.contains(room) ||
            (writer.forgot[room] ?? 0) > captured ||
            (writer.history[room] ?? 0) > captured) {
          return;
        }
        await db.transaction(
          (txn) => replaceStoredWindow(txn, page, closed.fresh),
        );
    }
  }
}
