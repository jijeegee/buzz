/// Device message store: every received room event, kept per server and
/// account in its own SQLite file. Capture only so far — nothing reads it
/// yet. Rules: docs/message-sync.md#message-store.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:sqflite/sqflite.dart';

import 'stored_event.dart';

const messageStoreSchemaVersion = 1;

/// Received events waiting for the writer; beyond this they are dropped and
/// the file is marked lossy.
const _queueLimit = 4096;

/// Rows written per transaction.
const _rowsPerTransaction = 512;

const _schema = [
  '''CREATE TABLE IF NOT EXISTS events(
  id TEXT PRIMARY KEY, kind INTEGER NOT NULL, pubkey TEXT NOT NULL,
  created_at INTEGER NOT NULL, room TEXT, root TEXT,
  broadcast INTEGER NOT NULL DEFAULT 0, raw TEXT NOT NULL)''',
  'CREATE INDEX IF NOT EXISTS events_room_order ON events(room, root, created_at, id)',
  'CREATE TABLE IF NOT EXISTS targets(id TEXT NOT NULL, target TEXT NOT NULL, PRIMARY KEY(target, id))',
  '''CREATE TABLE IF NOT EXISTS summaries(
  root TEXT PRIMARY KEY, room TEXT, created_at INTEGER NOT NULL, raw TEXT NOT NULL)''',
  '''CREATE TABLE IF NOT EXISTS coverage(
  room TEXT NOT NULL, stream TEXT NOT NULL,
  lo_at INTEGER NOT NULL, lo_id TEXT NOT NULL, hi_at INTEGER NOT NULL, hi_id TEXT NOT NULL)''',
  'CREATE INDEX IF NOT EXISTS coverage_stream ON coverage(room, stream)',
  'CREATE TABLE IF NOT EXISTS sync_state(key TEXT PRIMARY KEY, value TEXT NOT NULL)',
];

/// `https://host[:port]` for any relay URL, so every request to one server
/// shares one file.
String? messageStoreOrigin(String relayUrl) {
  final uri = Uri.tryParse(relayUrl.trim());
  if (uri == null || uri.host.isEmpty) return null;
  final scheme = switch (uri.scheme) {
    'wss' || 'https' => 'https',
    'ws' || 'http' => 'http',
    _ => null,
  };
  if (scheme == null) return null;
  final host = uri.host.toLowerCase();
  return uri.hasPort ? '$scheme://$host:${uri.port}' : '$scheme://$host';
}

String _fileStem(String origin, String account) {
  final digest = sha256.convert(utf8.encode(origin)).bytes.take(8);
  final hex = digest.map((b) => b.toRadixString(16).padLeft(2, '0')).join();
  return '$hex-$account';
}

/// The scope a receive belongs to, taken when the session or request starts.
/// A lease from before its account signed out writes nothing.
@immutable
class MessageStoreLease {
  const MessageStoreLease._(this.stem, this.account, this.epoch);

  final String stem;
  final String account;
  final int epoch;

  @override
  bool operator ==(Object other) =>
      other is MessageStoreLease &&
      other.stem == stem &&
      other.account == account &&
      other.epoch == epoch;

  @override
  int get hashCode => Object.hash(stem, account, epoch);
}

class _Writer {
  _Writer(this.open);

  final Future<Database> Function() open;
  final List<Map<String, dynamic>> queue = [];
  Future<Database>? database;
  Future<void> drained = Future.value();
  bool scheduled = false;
  bool lossy = false;
  bool stopped = false;
}

class MessageStore {
  MessageStore({required this.factory, required this.directory});

  /// Read when a file is first opened: the platform factory is not there
  /// in widget tests that never write.
  final DatabaseFactory Function() factory;

  /// Where the files live; resolved once, lazily.
  final Future<String> Function() directory;

  final Map<String, _Writer> _writers = {};
  final Map<String, int> _epochs = {};

  /// File deletions in progress per account; a new file waits for them.
  final Map<String, Future<void>> _forgetting = {};
  Future<String>? _dir;

  Future<String> _directory() => _dir ??= directory();

  Future<String> pathOf(MessageStoreLease lease) async =>
      '${await _directory()}/${lease.stem}.db';

  /// The scope for events received from [relayUrl] as [account].
  MessageStoreLease? lease(String relayUrl, String? account) {
    final origin = messageStoreOrigin(relayUrl);
    final pubkey = account?.trim().toLowerCase();
    if (origin == null ||
        pubkey == null ||
        !RegExp(r'^[0-9a-f]{64}$').hasMatch(pubkey)) {
      return null;
    }
    return MessageStoreLease._(
      _fileStem(origin, pubkey),
      pubkey,
      _epochs[pubkey] ?? 0,
    );
  }

  /// Queues received relay events (JSON maps) for [lease]'s file. Never
  /// waits for the disk; what to keep is decided when the queue is written.
  void submit(MessageStoreLease? lease, Iterable<Object?> events) {
    if (lease == null || (_epochs[lease.account] ?? 0) != lease.epoch) return;
    final writer = _writers.putIfAbsent(
      lease.stem,
      () => _Writer(() => _open(lease)),
    );
    for (final event in events) {
      if (event is! Map<String, dynamic>) continue;
      if (writer.queue.length >= _queueLimit) {
        writer.lossy = true;
        break;
      }
      writer.queue.add(event);
    }
    if (!writer.scheduled && writer.queue.isNotEmpty) {
      writer.scheduled = true;
      writer.drained = writer.drained.then((_) => _drain(writer));
    }
  }

  /// Completes once everything submitted so far is written (tests).
  @visibleForTesting
  Future<void> flushed() => Future.wait(_writers.values.map((w) => w.drained));

  Future<Database> _open(MessageStoreLease lease) async {
    await _forgetting[lease.account];
    await Directory(await _directory()).create(recursive: true);
    final db = await factory().openDatabase(
      await pathOf(lease),
      options: OpenDatabaseOptions(
        version: messageStoreSchemaVersion,
        onConfigure: (db) async {
          await db.rawQuery('PRAGMA journal_mode=WAL');
        },
        onCreate: (db, _) async {
          for (final statement in _schema) {
            await db.execute(statement);
          }
        },
        onDowngrade: (db, from, to) =>
            throw StateError('message store version $from is newer than $to'),
      ),
    );
    return db;
  }

  Future<void> _drain(_Writer writer) async {
    try {
      while (writer.queue.isNotEmpty && !writer.stopped) {
        final take = writer.queue.length < _rowsPerTransaction
            ? writer.queue.length
            : _rowsPerTransaction;
        final events = writer.queue.sublist(0, take);
        writer.queue.removeRange(0, take);
        final rows = events.map(classifyStoredEvent).nonNulls.toList();
        final lossy = writer.lossy;
        writer.lossy = false;
        if (rows.isEmpty && !lossy) continue;
        final db = await (writer.database ??= writer.open());
        if (writer.stopped) return;
        await db.transaction((txn) async {
          final batch = txn.batch();
          for (final row in rows) {
            _write(batch, row);
          }
          if (lossy) {
            batch.rawInsert(
              "INSERT OR REPLACE INTO sync_state(key, value) VALUES ('lossy', '1')",
            );
          }
          await batch.commit(noResult: true);
        });
      }
    } catch (error) {
      debugPrint('[MessageStore] write failed: $error');
      writer.queue.clear();
      writer.lossy = true;
    } finally {
      writer.scheduled = false;
    }
  }

  void _write(Batch batch, StoredEvent row) {
    switch (row) {
      case StoredRow():
        batch.rawInsert(
          'INSERT OR IGNORE INTO events(id, kind, pubkey, created_at, room, root, broadcast, raw) '
          'VALUES (?, ?, ?, ?, ?, ?, ?, ?)',
          [
            row.id,
            row.kind,
            row.pubkey,
            row.createdAt,
            row.room,
            row.root,
            row.broadcast ? 1 : 0,
            row.raw,
          ],
        );
        for (final target in row.targets) {
          batch.rawInsert(
            'INSERT OR IGNORE INTO targets(id, target) VALUES (?, ?)',
            [row.id, target],
          );
        }
      case StoredSummary():
        batch.rawInsert(
          'INSERT INTO summaries(root, room, created_at, raw) VALUES (?, ?, ?, ?) '
          'ON CONFLICT(root) DO UPDATE SET room = excluded.room, '
          'created_at = excluded.created_at, raw = excluded.raw '
          'WHERE excluded.created_at >= summaries.created_at',
          [row.root, row.room, row.createdAt, row.raw],
        );
    }
  }

  /// Ends every lease of [account] at once, then stops its writers and
  /// deletes its files. Callers need not wait: nothing more is written for
  /// the old leases, and a new file waits for the deletion.
  Future<void> forget(String account) {
    final pubkey = account.trim().toLowerCase();
    _epochs[pubkey] = (_epochs[pubkey] ?? 0) + 1;
    final suffix = '-$pubkey';
    final stopping = [
      for (final stem
          in _writers.keys.where((s) => s.endsWith(suffix)).toList())
        _writers.remove(stem)!
          ..stopped = true
          ..queue.clear(),
    ];
    final previous = _forgetting[pubkey] ?? Future<void>.value();
    final done = previous.then((_) => _delete(stopping, suffix));
    _forgetting[pubkey] = done;
    return done;
  }

  Future<void> _delete(List<_Writer> stopping, String suffix) async {
    for (final writer in stopping) {
      try {
        await writer.drained;
        await (await writer.database)?.close();
      } catch (error) {
        debugPrint('[MessageStore] close failed: $error');
      }
    }
    final Directory dir;
    try {
      dir = Directory(await _directory());
      if (!await dir.exists()) return;
    } catch (_) {
      return;
    }
    try {
      await for (final entry in dir.list()) {
        final name = entry.uri.pathSegments.last;
        if (name.split('.').first.endsWith(suffix)) await entry.delete();
      }
    } catch (error) {
      debugPrint('[MessageStore] could not delete files: $error');
    }
  }
}
