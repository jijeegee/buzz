/// Device message store: every received room event, kept per server and
/// account in its own SQLite file, with the spans known complete. Nothing
/// shows it yet. Rules: docs/message-sync.md#message-store.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:sqflite/sqflite.dart';

import 'coverage.dart' as spans;
import 'purge.dart' as purge;
import 'room_head.dart';
import 'thread_head.dart';

export 'clock_check.dart' show clockTrusted, noteServerDate;
import 'stored_event.dart';

const messageStoreSchemaVersion = 1;

/// Receives waiting for the writer, as on desktop: one submit is one input,
/// however many events it holds. Beyond this one is dropped and the file is
/// marked lossy.
const _queueInputs = 4096;

/// Events waiting for the writer, a memory guard well above any one answer.
const _queueEvents = 50000;

/// Rows written per transaction.
const _rowsPerTransaction = 512;

/// How long a store open waits for queued writes before asking the relay.
const _drainWait = Duration(milliseconds: 500);

const _schema = [
  '''CREATE TABLE IF NOT EXISTS events(
  id TEXT PRIMARY KEY, kind INTEGER NOT NULL, pubkey TEXT NOT NULL,
  created_at INTEGER NOT NULL, room TEXT, root TEXT,
  broadcast INTEGER NOT NULL DEFAULT 0, raw TEXT NOT NULL)''',
  'CREATE INDEX IF NOT EXISTS events_room_order ON events(room, root, created_at, id)',
  'CREATE INDEX IF NOT EXISTS events_job_rooms ON events(room) WHERE kind BETWEEN 43001 AND 43006',
  'CREATE INDEX IF NOT EXISTS events_room_rows ON events(room, created_at DESC, id) WHERE root IS NULL OR broadcast = 1',
  'CREATE TABLE IF NOT EXISTS targets(id TEXT NOT NULL, target TEXT NOT NULL, PRIMARY KEY(target, id))',
  '''CREATE TABLE IF NOT EXISTS summaries(
  root TEXT PRIMARY KEY, room TEXT, created_at INTEGER NOT NULL, raw TEXT NOT NULL)''',
  '''CREATE TABLE IF NOT EXISTS coverage(
  room TEXT NOT NULL, stream TEXT NOT NULL,
  lo_at INTEGER NOT NULL, lo_id TEXT NOT NULL, hi_at INTEGER NOT NULL, hi_id TEXT NOT NULL)''',
  'CREATE INDEX IF NOT EXISTS coverage_stream ON coverage(room, stream)',
  'CREATE TABLE IF NOT EXISTS sync_state(key TEXT PRIMARY KEY, value TEXT NOT NULL)',
  ...purge.roomMarksSchema,
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

String _originHash(String origin) => sha256
    .convert(utf8.encode(origin))
    .bytes
    .take(8)
    .map((b) => b.toRadixString(16).padLeft(2, '0'))
    .join();

String _fileStem(String origin, String account) =>
    '${_originHash(origin)}-$account';

final _pubkey = RegExp(r'^[0-9a-f]{64}$');

/// The scope a receive belongs to, taken when the session or request starts.
/// A lease from before its account signed out of that server writes nothing.
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

/// A point in a file's input sequence, with the scope it was taken under
/// (docs/message-sync.md#forgetting-a-room): a coverage span or membership
/// list carrying it is refused once the scope ends, and for rooms forgotten
/// after it.
typedef StoreCapture = ({MessageStoreLease lease, int sequence});

/// A coverage span, queued behind the events received before it; its
/// capture began at [captured].
class _CoverageOp {
  const _CoverageOp(this.rooms, this.from, this.to, this.captured);

  final List<String> rooms;
  final int from;
  final int to;
  final int captured;
}

/// Apply this membership list, fetched from capture [captured] on.
class _ForgetOp {
  const _ForgetOp(this.keep, this.captured);

  final Set<String> keep;
  final int captured;
}

/// An event that may be a relay notice signed by [relayKey]
/// (docs/message-sync.md#forgetting-a-room).
class _NoticeOp {
  const _NoticeOp(this.event, this.relayKey);

  final Map<String, dynamic> event;
  final String relayKey;
}

class _Writer {
  _Writer(this.open, this.account);

  final Future<Database> Function() open;

  /// The account whose file this is: a notice naming it is a rejoin.
  final String account;

  /// Per room, the sequence number of the input that first wrote it while
  /// this writer runs, and of the last input that forgot it. Captures never
  /// outlive the writer, so nothing older is needed.
  final Map<String, int> born = {};
  final Map<String, int> forgot = {};

  /// Submitted event lists, [_CoverageOp]s, [_ForgetOp]s and [_NoticeOp]s,
  /// in the order they were received.
  final List<Object> queue = [];

  /// Events of the first list already written.
  int headOffset = 0;

  /// Events in [queue] not yet written.
  int queuedEvents = 0;

  void clearQueue() {
    queue.clear();
    headOffset = 0;
    queuedEvents = 0;
  }

  Future<Database>? database;

  /// The file's rooms, loaded with the first write after it opens.
  purge.StoredRooms? rooms;

  Future<void> drained = Future.value();
  bool scheduled = false;
  bool lossy = false;
  bool stopped = false;

  /// When (unix seconds) something received was last not written: a span
  /// starting at or before then may miss it.
  int lostAt = 0;

  void noteLoss() {
    lossy = true;
    lostAt = DateTime.now().millisecondsSinceEpoch ~/ 1000;
  }

  /// Inputs ever queued (the file's sequence: each input takes the next
  /// number), and how many of them are written (or dropped).
  int queued = 0;
  int passed = 0;
  final List<(int, Completer<void>)> _waiters = [];

  void enqueue(Object input) {
    queue.add(input);
    queued++;
  }

  /// Completes once every input queued so far is written or dropped, however
  /// much is queued after it.
  Future<void> barrier() {
    if (passed >= queued) return Future.value();
    final waiter = Completer<void>();
    _waiters.add((queued, waiter));
    return waiter.future;
  }

  void advance(int count) {
    passed = (passed + count).clamp(0, queued);
    _waiters.removeWhere((entry) {
      if (entry.$1 > passed) return false;
      entry.$2.complete();
      return true;
    });
  }
}

class MessageStore {
  MessageStore({
    required this.factory,
    required this.directory,
    this.drainWait = _drainWait,
  });

  /// Read when a file is first opened: the platform factory is not there
  /// in widget tests that never write.
  final DatabaseFactory Function() factory;

  /// Where the files live; resolved once, lazily.
  final Future<String> Function() directory;

  /// How long a store open waits for queued writes before giving up.
  final Duration drainWait;

  final Map<String, _Writer> _writers = {};
  final Map<String, int> _epochs = {};

  /// Each server's signing key (NIP-11 `self`), by origin hash: only its
  /// notices may forget a room.
  final Map<String, String> _relayKeys = {};

  /// Epochs and file deletions in progress, per file stem (server and
  /// account); a new file waits for its deletion.
  final Map<String, Future<void>> _forgetting = {};
  Future<String>? _dir;

  Future<String> _directory() => _dir ??= directory();

  Future<String> pathOf(MessageStoreLease lease) async =>
      '${await _directory()}/${lease.stem}.db';

  /// The scope for events received from [relayUrl] as [account].
  MessageStoreLease? lease(String relayUrl, String? account) {
    final origin = messageStoreOrigin(relayUrl);
    final pubkey = account?.trim().toLowerCase();
    if (origin == null || pubkey == null || !_pubkey.hasMatch(pubkey)) {
      return null;
    }
    return MessageStoreLease._(
      _fileStem(origin, pubkey),
      pubkey,
      _epochs[_fileStem(origin, pubkey)] ?? 0,
    );
  }

  /// Queues received relay events (JSON maps) for [lease]'s file. Never
  /// waits for the disk; what to keep is decided when the queue is written.
  /// Relay notices among them may forget a room once the server's signing
  /// key is known (docs/message-sync.md#forgetting-a-room).
  void submit(MessageStoreLease? lease, Iterable<Object?> events) {
    final writer = _writerFor(lease);
    if (writer == null) return;
    final input = events.whereType<Map<String, dynamic>>().toList();
    if (input.isEmpty) return;
    final relayKey = _relayKeys[lease!.stem.split('-').first];
    var plain = <Map<String, dynamic>>[];
    for (final event in input) {
      if (relayKey == null ||
          !purge.mayBeRoomNotice(event, relayKey, writer.account)) {
        plain.add(event);
        continue;
      }
      _enqueueEvents(writer, plain, plain.length);
      plain = [];
      _enqueueEvents(writer, _NoticeOp(event, relayKey), 1);
    }
    _enqueueEvents(writer, plain, plain.length);
    _schedule(writer);
  }

  void _enqueueEvents(_Writer writer, Object input, int count) {
    if (count == 0) return;
    if (writer.queue.length >= _queueInputs ||
        writer.queuedEvents + count > _queueEvents) {
      writer.noteLoss();
    } else {
      writer.enqueue(input);
      writer.queuedEvents += count;
    }
  }

  /// Notes the signing key (NIP-11 `self`) of [lease]'s server: relay
  /// notices signed by it may forget a room; none are trusted before it is
  /// known.
  void noteRelayKey(MessageStoreLease? lease, String? key) {
    final pubkey = key?.trim().toLowerCase();
    if (lease == null || pubkey == null || !_pubkey.hasMatch(pubkey)) return;
    _relayKeys[lease.stem.split('-').first] = pubkey;
  }

  /// A capture of [lease]'s file now; `null` once the lease has ended.
  StoreCapture? capture(MessageStoreLease? lease) {
    final writer = _writerFor(lease);
    return writer == null ? null : (lease: lease!, sequence: writer.queued);
  }

  /// Applies [keep], the account's full membership list from a successful
  /// fetch, to the rooms stored under [capture]'s file
  /// (docs/message-sync.md#forgetting-a-room). [capture] is taken before
  /// that fetch; refused (false) when its scope is not [current] or has
  /// ended (signed out), and for an empty list. Queued behind earlier writes.
  bool forgetRoomsExcept(
    StoreCapture? capture,
    MessageStoreLease? current,
    Set<String> keep,
  ) {
    if (capture == null || capture.lease != current || keep.isEmpty) {
      return false;
    }
    final writer = _writerFor(capture.lease);
    if (writer == null) return false;
    writer.enqueue(_ForgetOp(Set.of(keep), capture.sequence));
    _schedule(writer);
    return true;
  }

  /// Records that every event of [rooms] in `[from, to]` has been received,
  /// by a capture that began at [captured] (default: now). Applied once the
  /// events submitted before it are written, dropped if any of them was
  /// lost, and refused for rooms forgotten after [captured].
  void extendCoverage(
    MessageStoreLease? lease,
    List<String> rooms,
    int from,
    int to, {
    int? captured,
  }) {
    if (rooms.isEmpty || from > to) return;
    final writer = _writerFor(lease);
    if (writer == null) return;
    final at = captured ?? writer.queued;
    writer.enqueue(_CoverageOp(List.of(rooms), from, to, at));
    _schedule(writer);
  }

  /// The end of each room's latest span begun before [before], for rooms
  /// that have one. Empty when nothing was stored yet.
  Future<Map<String, int>> resumePoints(
    MessageStoreLease? lease,
    List<String> rooms,
    int before,
  ) async {
    if (rooms.isEmpty) return const {};
    final points = await _read(
      lease,
      'resume points',
      (db) => spans.coverageResumePoints(db, rooms, before),
    );
    return points ?? const {};
  }

  /// [room]'s newest window as a channel-window response, only when it is
  /// certain (docs/message-sync.md#opening-a-room-from-the-store).
  Future<List<Map<String, dynamic>>?> certainRoomHead(
    MessageStoreLease? lease, {
    required String room,
    required List<int> kinds,
    required int limit,
    required int liveAt,
  }) async {
    if (!await _drainedFor(lease)) return null;
    final head = await _read(
      lease,
      'room head',
      // One transaction: the paged reads see one state of the file.
      (db) => db.transaction(
        (txn) => readRoomHead(
          txn,
          room: room,
          kinds: kinds,
          limit: limit,
          liveAt: liveAt,
        ),
      ),
    );
    return head != null && head.certain ? head.toResponse(room) : null;
  }

  /// [root]'s newest replies in [room] as a thread-window response, only
  /// when certain (docs/message-sync.md#opening-a-thread-from-the-store).
  Future<List<Map<String, dynamic>>?> certainThreadHead(
    MessageStoreLease? lease, {
    required String room,
    required String root,
    required List<int> kinds,
    required int limit,
    required int liveAt,
  }) async {
    if (!await _drainedFor(lease)) return null;
    final head = await _read(
      lease,
      'thread head',
      // One transaction: the paged reads see one state of the file.
      (db) => db.transaction(
        (txn) => readThreadHead(
          txn,
          room: room,
          root: root,
          kinds: kinds,
          limit: limit,
          liveAt: liveAt,
        ),
      ),
    );
    return head != null && head.certain ? head.toResponse(room, root) : null;
  }

  /// Whether everything received for [lease] so far is written and nothing
  /// was lost, waiting at most [drainWait] (and not for what arrives later)
  /// (docs/message-sync.md#opening-a-room-from-the-store).
  Future<bool> _drainedFor(MessageStoreLease? lease) async {
    final writer = _writerFor(lease);
    if (writer == null || writer.stopped || writer.lossy) return false;
    try {
      await writer.barrier().timeout(drainWait);
    } on TimeoutException {
      return false;
    }
    return !writer.stopped && !writer.lossy;
  }

  /// Runs [read] on [lease]'s file; `null` when there is no file yet or it
  /// fails. Never creates the file.
  Future<T?> _read<T>(
    MessageStoreLease? lease,
    String what,
    Future<T> Function(Database db) read,
  ) async {
    try {
      final writer = _writerFor(lease);
      if (writer == null) return null;
      if (writer.database == null &&
          !await File(await pathOf(lease!)).exists()) {
        return null;
      }
      final db = await (writer.database ??= writer.open());
      if (writer.stopped) return null;
      return await read(db);
    } catch (error) {
      debugPrint('[MessageStore] $what failed: $error');
      return null;
    }
  }

  _Writer? _writerFor(MessageStoreLease? lease) {
    if (lease == null || (_epochs[lease.stem] ?? 0) != lease.epoch) {
      return null;
    }
    return _writers.putIfAbsent(
      lease.stem,
      () => _Writer(() => _open(lease), lease.account),
    );
  }

  void _schedule(_Writer writer) {
    if (!writer.scheduled && writer.queue.isNotEmpty) {
      writer.scheduled = true;
      writer.drained = writer.drained.then((_) => _drain(writer));
    }
  }

  /// Completes once everything submitted so far is written (tests).
  @visibleForTesting
  void noteLossForTest(MessageStoreLease lease) =>
      _writerFor(lease)?.noteLoss();

  @visibleForTesting
  Future<void> flushed() => Future.wait(_writers.values.map((w) => w.drained));

  Future<Database> _open(MessageStoreLease lease) async {
    await _forgetting[lease.stem];
    await Directory(await _directory()).create(recursive: true);
    final db = await factory().openDatabase(
      await pathOf(lease),
      options: OpenDatabaseOptions(
        version: messageStoreSchemaVersion,
        onConfigure: (db) async {
          await db.rawQuery('PRAGMA journal_mode=WAL');
        },
        // Every open, not only the first: indexes added later reach files
        // an older build created.
        onOpen: (db) async {
          for (final statement in _schema) {
            await db.execute(statement);
          }
          await spans.dropStaleCoverage(db);
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
        final head = writer.queue.first;
        final coverage = head is _CoverageOp ? head : null;
        final forget = head is _ForgetOp ? head : null;
        final notice = head is _NoticeOp ? head : null;
        final List<Map<String, dynamic>> events;
        final int taken;
        if (coverage != null || forget != null || notice != null) {
          writer.queue.removeAt(0);
          events = notice == null
              ? const <Map<String, dynamic>>[]
              : [notice.event];
          writer.queuedEvents -= events.length;
          taken = 1;
        } else {
          events = [];
          var inputs = 0;
          // Whole lists up to a transaction's rows; a long one is split.
          while (inputs < writer.queue.length &&
              events.length < _rowsPerTransaction) {
            final input = writer.queue[inputs];
            if (input is! List<Map<String, dynamic>>) break;
            final end = math.min(
              input.length,
              writer.headOffset + _rowsPerTransaction - events.length,
            );
            events.addAll(input.getRange(writer.headOffset, end));
            if (end < input.length) {
              writer.headOffset = end;
              break;
            }
            writer.headOffset = 0;
            inputs++;
          }
          writer.queue.removeRange(0, inputs);
          writer.queuedEvents -= events.length;
          taken = inputs;
        }
        final rows = events.map(classifyStoredEvent).nonNulls.toList();
        // The sequence number of the last input in hand (of the head list,
        // when only part of it is).
        final sequence = writer.passed + math.max<int>(taken, 1);
        for (final row in rows) {
          final room = switch (row) {
            StoredRow() => row.room,
            StoredSummary() => row.room,
          };
          if (room != null) writer.born.putIfAbsent(room, () => sequence);
        }
        final lossy = writer.lossy;
        writer.lossy = false;
        if (rows.isEmpty && !lossy && coverage == null && forget == null) {
          writer.advance(taken);
          continue;
        }
        final db = await (writer.database ??= writer.open());
        if (writer.stopped) return;
        final known = writer.rooms ??= await purge.StoredRooms.load(db);
        final forgotten = <String>[];
        await db.transaction((txn) async {
          // A notice forgets the rows received before it, then is written.
          final row = rows.firstOrNull;
          if (notice != null && row is StoredRow) {
            final applies = purge.roomNotice(
              row,
              writer.account,
              notice.relayKey,
            );
            if (applies != null &&
                await purge.applyRoomNotice(txn, known, row, applies)) {
              forgotten.add(row.room!);
            }
          }
          final batch = txn.batch();
          for (final row in rows) {
            final room = switch (row) {
              StoredRow() => row.room,
              StoredSummary() => row.room,
            };
            if (room != null) known.note(room);
            _write(batch, row);
          }
          if (lossy) {
            batch.rawInsert(
              "INSERT OR REPLACE INTO sync_state(key, value) VALUES ('lossy', '1')",
            );
            batch.rawDelete('DELETE FROM coverage');
          }
          await batch.commit(noResult: true);
          if (coverage != null && !lossy && coverage.from > writer.lostAt) {
            // Not for a room forgotten after the span's capture began.
            final rooms = coverage.rooms
                .where(
                  (room) => (writer.forgot[room] ?? 0) <= coverage.captured,
                )
                .toList();
            rooms.forEach(known.note);
            await spans.extendCoverage(txn, rooms, coverage.from, coverage.to);
          }
        });
        if (forget != null) {
          // Its own transactions, a chunk of rooms each.
          forgotten.addAll(
            await purge.applyList(
              db,
              known,
              forget.keep,
              (room) => (writer.born[room] ?? 0) > forget.captured,
            ),
          );
        }
        if (coverage != null) {
          for (final room in coverage.rooms) {
            writer.born.putIfAbsent(room, () => sequence);
          }
        }
        for (final room in forgotten) {
          writer.forgot[room] = sequence;
        }
        writer.advance(taken);
      }
    } catch (error) {
      debugPrint('[MessageStore] write failed: $error');
      writer.rooms = null;
      writer.clearQueue();
      writer.noteLoss();
    } finally {
      writer.scheduled = false;
      // Dropped on failure or stop: nothing left to wait for.
      if (writer.queue.isEmpty || writer.stopped) writer.advance(writer.queued);
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

  /// Ends every lease of [account] on [relayUrl] at once, then stops that
  /// file's writer and deletes the file. The account's files from other
  /// servers are kept and keep recording. Callers need not wait: nothing more
  /// is written for the old leases, and a new file waits for the deletion.
  Future<void> forget(String relayUrl, String account) {
    final origin = messageStoreOrigin(relayUrl);
    if (origin == null) return Future.value();
    final stem = _fileStem(origin, account.trim().toLowerCase());
    _epochs[stem] = (_epochs[stem] ?? 0) + 1;
    final stopping = [
      if (_writers.remove(stem) case final writer?)
        writer
          ..stopped = true
          ..clearQueue()
          ..advance(writer.queued),
    ];
    final previous = _forgetting[stem] ?? Future<void>.value();
    final done = previous.then((_) => _delete(stopping, stem));
    _forgetting[stem] = done;
    return done;
  }

  Future<void> _delete(List<_Writer> stopping, String stem) async {
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
        if (name.split('.').first == stem) await entry.delete();
      }
    } catch (error) {
      debugPrint('[MessageStore] could not delete files: $error');
    }
  }
}
