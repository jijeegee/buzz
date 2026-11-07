/// The store side of checked windows: a room or thread window read for the
/// check (rows of the page's kinds and top-level rule, deleted rows
/// dropped, rows only the push inbox wrote left out), and the replacement
/// of a fetched page's key range.
library;

import 'dart:convert';

import 'package:sqflite/sqflite.dart';

import 'room_head.dart';
import 'thread_head.dart';

/// The window a check or page asks for: [root] `null` for a room window,
/// [before] `null` for the newest.
typedef StoreWindowKey = ({
  String room,
  String? root,
  List<int> kinds,
  (int, String)? before,
});

/// A window as the store holds it, for the check.
class StoredCheckWindow {
  StoredCheckWindow({
    required this.window,
    required this.rows,
    required this.extra,
    required this.oldest,
    required this.fullyStored,
  });

  final StoreWindowKey window;

  /// Newest first.
  final List<Map<String, dynamic>> rows;

  /// Overlays (and, for a room, thread summaries) carried with the rows.
  final List<Map<String, dynamic>> extra;

  /// `(created_at, id)` of the oldest row.
  final (int, String)? oldest;

  /// Whether a check can stand for the page: the store holds as many rows
  /// as asked, or the room's (thread's) start; and no catch-up left the
  /// room behind since its newest page.
  final bool fullyStored;

  /// The window in the shape of the relay's page; [hasMore] from the relay.
  List<Map<String, dynamic>> toResponse({required bool hasMore}) {
    final room = window.room;
    final root = window.root;
    final before = window.before;
    final cursor = hasMore ? oldest : null;
    final next = cursor == null
        ? null
        : {'created_at': cursor.$1, 'id': cursor.$2};
    return [
      ...rows,
      ...extra,
      {
        'id': '0' * 64,
        'pubkey': '0' * 64,
        'sig': '0' * 128,
        'kind': root == null ? 39006 : 39007,
        'created_at': 0,
        'tags': root == null
            ? [
                [
                  'd',
                  '${room.toLowerCase()}:'
                      '${before == null ? 'head' : '${before.$1}:${before.$2.toLowerCase()}'}',
                ],
                ['h', room],
              ]
            : [
                ['h', room],
                ['e', root],
              ],
        'content': jsonEncode({
          if (root != null) ...{'version': 1, 'direction': 'older'},
          'has_more': cursor != null,
          'next_cursor': next,
        }),
      },
    ];
  }
}

String _startKey(String room, String? root) =>
    'window_start:$room:${root ?? ''}';

String _refetchKey(String room) => 'window_refetch:$room';

/// Reads [window]'s newest [limit] rows (below its cursor) for the check.
Future<StoredCheckWindow> readCheckWindow(
  DatabaseExecutor db,
  StoreWindowKey window, {
  required int limit,
}) async {
  final root = window.root;
  final List<Map<String, dynamic>> rows;
  final List<Map<String, dynamic>> extra;
  if (root == null) {
    final head = await readRoomHead(
      db,
      room: window.room,
      kinds: window.kinds,
      limit: limit,
      liveAt: null,
      before: window.before,
    );
    rows = head.rows;
    extra = [...head.aux, ...head.summaries];
  } else {
    final head = await readThreadHead(
      db,
      room: window.room,
      root: root,
      kinds: window.kinds,
      limit: limit,
      liveAt: null,
      before: window.before,
    );
    rows = head.rows;
    extra = head.overlays;
  }
  final last = rows.lastOrNull;
  final oldest = last == null
      ? null
      : (last['created_at'] as int, last['id'] as String);
  final markers = await db.rawQuery(
    'SELECT key FROM sync_state WHERE key IN (?, ?)',
    [_startKey(window.room, root), _refetchKey(window.room)],
  );
  final keys = {for (final row in markers) row['key']};
  final fullyStored =
      !keys.contains(_refetchKey(window.room)) &&
      (rows.length >= limit || keys.contains(_startKey(window.room, root)));
  return StoredCheckWindow(
    window: window,
    rows: rows,
    extra: extra,
    oldest: oldest,
    fullyStored: fullyStored,
  );
}

/// A fetched page of [window]: its row [ids], its oldest row, and whether
/// the relay has more below it.
typedef StoreWindowPage = ({
  StoreWindowKey window,
  Set<String> ids,
  (int, String)? oldest,
  bool hasMore,
});

/// Makes the store's rows of [page]'s key range the page's: rows of its
/// kinds (and top-level rule, for a room) from its oldest row (down to the
/// start when it has no more) up to its cursor (or the newest) that it
/// lacks were deleted on the relay or are not this reader's, and go, unless
/// written since the request went out ([fresh]). A page with no more marks
/// the start as stored; a room's newest page clears what a catch-up left.
Future<void> replaceStoredWindow(
  Transaction txn,
  StoreWindowPage page,
  Set<String> fresh,
) async {
  final window = page.window;
  if (window.kinds.isEmpty) return;
  final oldest = page.hasMore ? page.oldest : null;
  if (page.hasMore && oldest == null) return;
  final bounds = <String>[];
  final args = <Object>[];
  if (oldest != null) {
    // At or newer than the oldest row, in relay order.
    bounds.add('AND (created_at > ? OR (created_at = ? AND id <= ?))');
    args.addAll([oldest.$1, oldest.$1, oldest.$2]);
  }
  if (window.before case final before?) {
    bounds.add(storedOlderThan);
    args.addAll(storedOlderArgs(before)!);
  }
  final kinds = window.kinds.join(', ');
  final root = window.root;
  final candidates = root == null
      ? await txn.rawQuery(
          'SELECT id, root, raw FROM events WHERE room = ? AND kind IN ($kinds) '
          'AND (root IS NULL OR broadcast = 1) ${bounds.join(' ')}',
          [window.room, ...args],
        )
      : await txn.rawQuery(
          'SELECT id, NULL AS root, NULL AS raw FROM events WHERE room = ? '
          'AND kind IN ($kinds) AND root = ? ${bounds.join(' ')}',
          [window.room, root, ...args],
        );
  final doomed = <String>[];
  for (final candidate in candidates) {
    final id = candidate['id']! as String;
    if (page.ids.contains(id) || fresh.contains(id)) continue;
    // A broadcast reply is a window row only as a direct reply.
    final replyTo = candidate['root'] as String?;
    if (replyTo != null) {
      final raw = candidate['raw'];
      Object? event;
      try {
        event = raw is String ? jsonDecode(raw) : null;
      } on FormatException {
        event = null;
      }
      if (event is! Map<String, dynamic> || storedParentOf(event) != replyTo) {
        continue;
      }
    }
    doomed.add(id);
  }
  final batch = txn.batch();
  for (final id in doomed) {
    batch.rawDelete('DELETE FROM events WHERE id = ?', [id]);
    batch.rawDelete('DELETE FROM targets WHERE id = ?', [id]);
    batch.rawDelete('DELETE FROM summaries WHERE root = ?', [id]);
  }
  if (!page.hasMore) {
    batch.rawInsert(
      "INSERT OR REPLACE INTO sync_state(key, value) VALUES (?, '1')",
      [_startKey(window.room, root)],
    );
  }
  if (root == null && window.before == null) {
    batch.rawDelete('DELETE FROM sync_state WHERE key = ?', [
      _refetchKey(window.room),
    ]);
  }
  await batch.commit(noResult: true);
}

/// A catch-up skipped or gave up on [rooms]: their stored windows are not
/// to be trusted by a check until the room's newest page is fetched.
Future<void> writeWindowsRefetch(
  DatabaseExecutor db,
  Iterable<String> rooms,
) async {
  final batch = db.batch();
  for (final room in rooms.toSet()) {
    batch.rawInsert(
      "INSERT OR REPLACE INTO sync_state(key, value) VALUES (?, '1')",
      [_refetchKey(room)],
    );
  }
  await batch.commit(noResult: true);
}
