import 'dart:async';
import 'dart:convert';

import 'package:sqflite/sqflite.dart';

import '../relay/nostr_models.dart';

/// Raw received events only. A stored event is never a proof of history coverage.
const messageStoreKinds = {
  5,
  7,
  9,
  9005,
  39005,
  40001,
  40002,
  40003,
  40008,
  40099,
  43001,
  43002,
  43003,
  43004,
  43005,
  43006,
  45001,
  45003,
  48100,
  48101,
  48102,
  48103,
};

String _relayKey(String relay) => relay
    .trim()
    .replaceFirst(RegExp(r'^wss:'), 'https:')
    .replaceFirst(RegExp(r'^ws:'), 'http:')
    .replaceFirst(RegExp(r'/+$'), '')
    .toLowerCase();

/// A receive context captured before starting network I/O.
class MessageStoreLease {
  MessageStoreLease._(this.relay, this.pubkey);
  final String relay;
  final String pubkey;
  bool _active = true;
}

/// Append-only, account/relay-scoped SQLite storage, independent of UI caches.
/// Clearing a scope fences both queued and already-running writes. No count or
/// age eviction; unknown schema versions fail without destroying originals.
class MessageStore {
  MessageStore(this.path, {DatabaseFactory? factory}) : _factory = factory;

  final Future<String> Function() path;
  final DatabaseFactory? _factory;
  Database? _db;
  Future<void> _tail = Future.value();
  final Set<MessageStoreLease> _leases = {};
  final Map<(String, String), int> _clearing = {};
  bool _closed = false;

  MessageStoreLease begin(String relay, String pubkey) {
    if (_closed) throw StateError('Message store is closed');
    final lease = MessageStoreLease._(_relayKey(relay), pubkey.toLowerCase());
    if (lease.relay.isEmpty || lease.pubkey.isEmpty) {
      throw ArgumentError('Message store requires relay and account');
    }
    // A rebuild racing logout may still see the old authenticated state. Its
    // receive context must not repopulate rows after the queued deletion.
    if (_clearing.containsKey((lease.relay, lease.pubkey))) {
      lease._active = false;
      return lease;
    }
    _leases.add(lease);
    return lease;
  }

  void retire(MessageStoreLease lease) {
    lease._active = false;
    _leases.remove(lease);
  }

  Future<T> _serial<T>(Future<T> Function() operation) {
    final result = _tail.then((_) => operation());
    // The caller receives the error; the ordering chain must remain usable.
    _tail = result.then<void>((_) {}, onError: (Object _, StackTrace _) {});
    return result;
  }

  Future<Database> _open() async {
    if (_db != null) return _db!;
    final db = await (_factory ?? databaseFactory).openDatabase(
      await path(),
      options: OpenDatabaseOptions(
        version: 1,
        onConfigure: (db) async {
          await db.execute('PRAGMA busy_timeout=5000');
          await db.rawQuery('PRAGMA journal_mode=WAL');
          await db.execute('PRAGMA secure_delete=ON');
        },
        onCreate: (db, _) async {
          await db.execute(
            'CREATE TABLE scopes(relay TEXT NOT NULL, account TEXT NOT NULL, needs_repair INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(relay, account))',
          );
          await db.execute(
            'CREATE TABLE events(relay TEXT NOT NULL, account TEXT NOT NULL, id TEXT NOT NULL, kind INTEGER NOT NULL, created_at INTEGER NOT NULL, raw_json TEXT NOT NULL, PRIMARY KEY(relay, account, id))',
          );
          await db.execute(
            'CREATE TABLE event_tags(relay TEXT NOT NULL, account TEXT NOT NULL, id TEXT NOT NULL, tag TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(relay, account, id, tag, value))',
          );
          await db.execute(
            'CREATE INDEX event_tag_target ON event_tags(relay, account, tag, value)',
          );
          await db.execute(
            'CREATE INDEX event_order ON events(relay, account, created_at DESC, id)',
          );
        },
        onUpgrade: (_, _, _) =>
            throw StateError('Unsupported message store schema'),
        onDowngrade: (_, _, _) =>
            throw StateError('Unsupported message store schema'),
      ),
    );
    _db = db;
    return db;
  }

  /// Store a bounded batch atomically; partial/failed writes cannot mark sync done.
  Future<void> put(MessageStoreLease lease, List<NostrEvent> events) {
    if (events.length > 256) {
      throw ArgumentError('Message store batch exceeds 256');
    }
    return _serial(() async {
      if (!lease._active || _closed) return;
      final db = await _open();
      if (!lease._active || _closed) return;
      await db.transaction((txn) async {
        await txn.rawInsert(
          'INSERT OR IGNORE INTO scopes(relay,account) VALUES(?,?)',
          [lease.relay, lease.pubkey],
        );
        final batch = txn.batch();
        for (final event in events.where(
          (event) => messageStoreKinds.contains(event.kind),
        )) {
          final rawJson = jsonEncode(event.toJson());
          batch.rawInsert('INSERT OR IGNORE INTO events VALUES(?,?,?,?,?,?)', [
            lease.relay,
            lease.pubkey,
            event.id,
            event.kind,
            event.createdAt,
            rawJson,
          ]);
          for (final tag in event.tags) {
            if (tag.length < 2 || !{'h', 'e', 'a'}.contains(tag[0])) continue;
            batch.rawInsert(
              'INSERT OR IGNORE INTO event_tags SELECT ?,?,?,?,? WHERE EXISTS(SELECT 1 FROM events WHERE relay=? AND account=? AND id=? AND raw_json=?)',
              [
                lease.relay,
                lease.pubkey,
                event.id,
                tag[0],
                tag[1],
                lease.relay,
                lease.pubkey,
                event.id,
                rawJson,
              ],
            );
          }
        }
        await batch.commit(noResult: true);
      });
    });
  }

  /// Raw lookup for store consumers/tests; never loads a whole conversation.
  Future<NostrEvent?> get(MessageStoreLease lease, String id) =>
      _serial(() async {
        if (!lease._active || _closed) return null;
        final db = await _open();
        if (!lease._active || _closed) return null;
        final rows = await db.rawQuery(
          'SELECT raw_json FROM events WHERE relay=? AND account=? AND id=?',
          [lease.relay, lease.pubkey, id],
        );
        return rows.isEmpty
            ? null
            : NostrEvent.fromJson(
                jsonDecode(rows.single['raw_json'] as String)
                    as Map<String, dynamic>,
              );
      });

  /// Invalidate synchronously, then delete under the same serialization boundary
  /// as writes. A late HTTP completion cannot recreate the signed-out scope.
  Future<void> clear(String relay, String pubkey) {
    final normalized = _relayKey(relay);
    final scope = (normalized, pubkey.toLowerCase());
    _clearing[scope] = (_clearing[scope] ?? 0) + 1;
    for (final lease in _leases.toList()) {
      if (lease.relay == normalized && lease.pubkey == pubkey.toLowerCase()) {
        retire(lease);
      }
    }
    return _serial(() async {
      final db = await _open();
      await db.transaction((txn) async {
        for (final table in ['event_tags', 'events', 'scopes']) {
          await txn.rawDelete(
            'DELETE FROM $table WHERE relay=? AND account=?',
            [normalized, pubkey.toLowerCase()],
          );
        }
      });
      await db.rawQuery('PRAGMA wal_checkpoint(TRUNCATE)');
    }).whenComplete(() {
      final remaining = (_clearing[scope] ?? 1) - 1;
      if (remaining == 0) {
        _clearing.remove(scope);
      } else {
        _clearing[scope] = remaining;
      }
    });
  }

  Future<void> close() {
    _closed = true;
    for (final lease in _leases.toList()) {
      retire(lease);
    }
    return _serial(() async {
      await _db?.close();
      _db = null;
    });
  }
}
