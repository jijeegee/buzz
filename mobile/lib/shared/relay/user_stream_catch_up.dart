/// The user stream's store catch-up: classes at the stream EOSE, batches,
/// the new-row rule and the counters built on it
/// (docs/message-sync.md#catch-up-at-the-stream-eose,
/// docs/message-sync.md#counters).
library;

import 'dart:convert';
import 'dart:math';

import 'package:flutter/foundation.dart';

import '../message_store/message_store.dart';
import 'room_bundle_coverage.dart';

/// Rooms one stream catch-up batch names.
const userStreamBatchSize = 128;

/// Skipped rooms one skip audit walks at most.
const userStreamAuditMax = 128;

/// The `liveMissed` window: from the previous part 1 plus this, to the
/// previous last frame minus [userStreamLiveMissedHighSecs].
const userStreamLiveMissedLowSecs = 5;
const userStreamLiveMissedHighSecs = 900;

/// A snapshot room's class at the stream EOSE.
enum UserStreamRoomClass { uncovered, equal, behind, ahead }

/// [resume]: the room's resume point (null: no coverage); [stored]: its
/// `room_seq.seq` (null: no row); [seq]: the snapshot's.
UserStreamRoomClass userStreamRoomClass({
  required int? resume,
  required int? stored,
  required int seq,
}) {
  if (resume == null) return UserStreamRoomClass.uncovered;
  if (stored == null || stored < seq) return UserStreamRoomClass.behind;
  if (stored == seq) return UserStreamRoomClass.equal;
  return UserStreamRoomClass.ahead;
}

/// Whether a room resuming at [resume] is too old to walk at [liveAt].
bool userStreamStale(int resume, int liveAt) =>
    resume - storeResumeLookbackSecs < liveAt - storeCatchUpMaxAgeSecs;

@immutable
class UserStreamBatch {
  const UserStreamBatch(this.rooms, this.since);

  final List<String> rooms;
  final int since;

  @override
  String toString() => 'UserStreamBatch($since, $rooms)';
}

/// Stale rooms first taken out, then the rest sorted by resume point, then
/// id, 128 per batch, each from its oldest resume point minus 1865 s.
({List<String> stale, List<UserStreamBatch> batches}) userStreamBatches(
  Map<String, int> resume,
  int liveAt,
) {
  final stale = <String>[];
  final fresh = <MapEntry<String, int>>[];
  for (final entry in resume.entries) {
    (userStreamStale(entry.value, liveAt)
        ? stale.add(entry.key)
        : fresh.add(entry));
  }
  stale.sort();
  fresh.sort(
    (a, b) => a.value != b.value
        ? a.value.compareTo(b.value)
        : a.key.compareTo(b.key),
  );
  return (
    stale: stale,
    batches: [
      for (var i = 0; i < fresh.length; i += userStreamBatchSize)
        UserStreamBatch([
          for (final entry in fresh.skip(i).take(userStreamBatchSize))
            entry.key,
        ], max(0, fresh[i].value - storeResumeLookbackSecs)),
    ],
  );
}

/// The new-row rule: inserted by the walk, not received live on this
/// connection, created at or before [liveAt] and at or after the start of
/// the room's newest segment begun before [liveAt] ([head]).
bool userStreamNewRow({
  required bool inserted,
  required bool receivedLive,
  required int createdAt,
  required int liveAt,
  required (int, int)? head,
}) =>
    inserted &&
    !receivedLive &&
    head != null &&
    createdAt <= liveAt &&
    createdAt >= head.$1;

/// Skipped rooms whose own walk start is within 7 days of [liveAt].
List<String> userStreamAuditEligible(Map<String, int> skipped, int liveAt) => [
  for (final MapEntry(key: room, value: resume) in skipped.entries)
    if (!userStreamStale(resume, liveAt)) room,
]..sort();

/// A uniform sample of at most [userStreamAuditMax] of [eligible].
List<String> userStreamAuditSample(List<String> eligible, Random random) {
  if (eligible.length <= userStreamAuditMax) return List.of(eligible);
  return (List.of(eligible)..shuffle(random)).take(userStreamAuditMax).toList()
    ..sort();
}

/// Skip verification of predicted skips: `(walked, newRows)` per room.
({int verified, int falseSkip}) userStreamSkipVerification(
  Iterable<(bool, int)> rooms,
) {
  var verified = 0;
  var falseSkip = 0;
  for (final (walked, newRows) in rooms) {
    if (!walked) continue;
    newRows > 0 ? falseSkip++ : verified++;
  }
  return (verified: verified, falseSkip: falseSkip);
}

/// The audit verdicts of [completed] (room -> its batch completed).
({int audited, int auditNew, int failed}) userStreamAuditVerdicts(
  Map<String, bool> completed,
  Map<String, int> newRows,
) {
  var audited = 0;
  var auditNew = 0;
  var failed = 0;
  for (final MapEntry(key: room, value: done) in completed.entries) {
    if (!done) {
      failed++;
      continue;
    }
    audited++;
    if ((newRows[room] ?? 0) > 0) auditNew++;
  }
  return (audited: audited, auditNew: auditNew, failed: failed);
}

/// The previous live connection, for `liveMissed`: its carried rooms, its
/// part 1 `created_at` and its last frame.
@immutable
class UserStreamLivePrev {
  const UserStreamLivePrev(this.rooms, this.eoseAt, this.lastFrameAt);

  final Set<String> rooms;
  final int eoseAt;
  final int lastFrameAt;

  String encode() => jsonEncode({
    'rooms': rooms.toList()..sort(),
    'eose': eoseAt,
    'last': lastFrameAt,
  });

  /// The bounds alone: what a span extension saves when the set is the same.
  String encodeLast() => jsonEncode({'eose': eoseAt, 'last': lastFrameAt});

  /// [last]: the newer bounds from [encodeLast], used when they belong to
  /// the same connection (same `eose`).
  static UserStreamLivePrev? decode(String? text, {String? last}) {
    if (text == null) return null;
    try {
      final json = jsonDecode(text);
      if (json case {
        'rooms': final List rooms,
        'eose': final int eose,
        'last': final int lastFrame,
      }) {
        var newest = lastFrame;
        if (last != null) {
          if (jsonDecode(last) case {
            'eose': final int lastEose,
            'last': final int at,
          } when lastEose == eose && at > newest) {
            newest = at;
          }
        }
        return UserStreamLivePrev(
          rooms.whereType<String>().toSet(),
          eose,
          newest,
        );
      }
    } on FormatException {
      return null;
    }
    return null;
  }
}

/// A new row of a walk, as `liveMissed` needs it.
typedef UserStreamNewRow = ({
  String room,
  int createdAt,
  int kind,
  String pubkey,
});

/// `liveRoomsChecked` and `liveMissed` for [prev]'s rooms whose batch
/// completed ([completed]); [newRows] already passed the new-row rule.
({int checked, int missed}) userStreamLiveMissed({
  required UserStreamLivePrev? prev,
  required Set<String> completed,
  required Iterable<UserStreamNewRow> newRows,
  required String relayPubkey,
}) {
  if (prev == null || prev.rooms.isEmpty) return (checked: 0, missed: 0);
  final checked = prev.rooms.where(completed.contains).toSet();
  final low = prev.eoseAt + userStreamLiveMissedLowSecs;
  final high = prev.lastFrameAt - userStreamLiveMissedHighSecs;
  var missed = 0;
  for (final row in newRows) {
    if (!checked.contains(row.room)) continue;
    if (row.createdAt < low || row.createdAt > high) continue;
    if (row.kind == 9005 &&
        row.pubkey.toLowerCase() == relayPubkey.toLowerCase()) {
      continue;
    }
    missed++;
  }
  return (checked: checked.length, missed: missed);
}

/// New rows per room of a completed walk, by the new-row rule.
List<UserStreamNewRow> userStreamNewRows(
  StoreWalk walk,
  Map<String, (int, int)> heads,
  int liveAt,
  bool Function(String id) receivedLive,
) => [
  for (final event in walk.inserted)
    if ((event['tags'], event['created_at'], event['id']) case (
      final List tags,
      final int createdAt,
      final String id,
    ))
      if (_roomTag(tags) case final room?)
        if (userStreamNewRow(
          inserted: true,
          receivedLive: receivedLive(id),
          createdAt: createdAt,
          liveAt: liveAt,
          head: heads[room],
        ))
          (
            room: room,
            createdAt: createdAt,
            kind: event['kind'] is int ? event['kind'] as int : -1,
            pubkey: event['pubkey'] is String ? event['pubkey'] as String : '',
          ),
];

String? _roomTag(List tags) {
  for (final tag in tags) {
    if (tag is List && tag.length >= 2 && tag[0] == 'h' && tag[1] is String) {
      return tag[1] as String;
    }
  }
  return null;
}

/// A catch-up the stream asks for: at its EOSE ([fallback] false), or the
/// mid-connection fallback of [rooms] with [liveAt] the CLOSED time.
@immutable
class UserStreamCatchUpJob {
  const UserStreamCatchUpJob({
    required this.mode,
    required this.rooms,
    required this.seqs,
    required this.epoch,
    required this.relayPubkey,
    required this.liveAt,
    required this.captured,
    int? coverageBefore,
    this.prev,
    this.fallback = false,
  }) : coverageBefore = coverageBefore ?? liveAt;

  /// `live`, `gated` or `shadow` (by name).
  final String mode;
  final List<String> rooms;
  final Map<String, int> seqs;
  final String epoch;
  final String relayPubkey;
  final int liveAt;
  final int? captured;

  /// Only coverage spans begun before this count for the resume points and
  /// the new-row base: at the stream EOSE, the connection's start (AUTH OK),
  /// so spans that chunks or the stream opened on this connection never
  /// hide the offline gap (docs/message-sync.md#catch-up-at-the-stream-eose).
  /// The fallback keeps [liveAt]: the stream's own span is its resume point.
  final int coverageBefore;
  final UserStreamLivePrev? prev;
  final bool fallback;
}

/// What a catch-up found, for tests and the counters.
class UserStreamCatchUpResult {
  final classes = <String, UserStreamRoomClass>{};

  /// Rooms walked as catch-ups, and as the skip audit.
  final walked = <String>[];
  final audited = <String>[];
  final stale = <String>[];
  final counts = <String, int>{};

  void add(String name, [int by = 1]) {
    if (by != 0) counts[name] = (counts[name] ?? 0) + by;
  }
}

/// Runs the stream's catch-ups on one store scope.
class UserStreamCatchUp {
  UserStreamCatchUp({
    required this.scope,
    required this.query,
    required this.switched,
    required this.receivedLive,
    Random? random,
  }) : _random = random ?? Random();

  final StoreScope scope;
  final Future<Object?> Function(List<Map<String, dynamic>> filters) query;
  final bool Function() switched;
  final bool Function(String id) receivedLive;
  final Random _random;

  /// [audit] false leaves the skip audit out (tests of the skip itself).
  Future<UserStreamCatchUpResult> run(
    UserStreamCatchUpJob job, {
    @visibleForTesting bool audit = true,
  }) async {
    final result = UserStreamCatchUpResult();
    final store = scope.store;
    final lease = scope.lease;
    final resume = await store.resumePoints(
      lease,
      job.rooms,
      job.coverageBefore,
    );
    final covered = job.rooms.where(resume.containsKey).toList();
    final vouch = StoreSeqVouch({
      for (final room in covered) room: ?job.seqs[room],
    }, job.epoch);
    var walk = covered;
    var predicted = const <String>[];
    var sampled = const <String>[];
    if (!job.fallback) {
      final stored = await store.roomSeqs(lease, covered, epoch: job.epoch);
      final ahead = <String, int>{};
      for (final room in job.rooms) {
        final seq = job.seqs[room] ?? 0;
        final cls = userStreamRoomClass(
          resume: resume[room],
          stored: stored[room]?.$1,
          seq: seq,
        );
        result.classes[room] = cls;
        if (cls == UserStreamRoomClass.ahead) ahead[room] = seq;
      }
      if (ahead.isNotEmpty) {
        debugPrint('[user-stream] rows ahead of the snapshot: ${ahead.length}');
        store.deleteAheadRoomSeqs(lease, ahead);
      }
      final equal = [
        for (final room in covered)
          if (result.classes[room] == UserStreamRoomClass.equal) room,
      ];
      switch (job.mode) {
        case 'shadow':
          result.add('predictedSkips', equal.length);
          return result;
        case 'gated':
          walk = [
            for (final room in covered)
              if (result.classes[room] != UserStreamRoomClass.equal) room,
          ];
          if (audit) {
            sampled = userStreamAuditSample(
              userStreamAuditEligible({
                for (final room in equal) room: resume[room]!,
              }, job.liveAt),
              _random,
            );
          }
          walk = [...walk, ...sampled];
        default:
          predicted = equal;
          result.add('predictedSkips', equal.length);
      }
    }
    final heads = await store.coverageHeads(lease, walk, job.coverageBefore);
    final plan = userStreamBatches({
      for (final room in walk) room: resume[room]!,
    }, job.liveAt);
    result.stale.addAll(plan.stale);
    final completed = <String, bool>{};
    final newRows = <UserStreamNewRow>[];
    for (final batch in plan.batches) {
      StoreWalk done;
      try {
        done = await walkStore(
          scope: scope,
          query: query,
          switched: switched,
          rooms: batch.rooms,
          since: batch.since,
          liveAt: job.liveAt,
          captured: job.captured,
          vouch: vouch,
          probe: true,
        );
      } catch (error) {
        debugPrint('[user-stream] catch-up batch failed: $error');
        done = StoreWalk.failed;
      }
      for (final room in batch.rooms) {
        completed[room] = done.completed;
      }
      for (final room in batch.rooms) {
        (sampled.contains(room) ? result.audited : result.walked).add(room);
      }
      if (done.completed) {
        newRows.addAll(
          userStreamNewRows(done, heads, job.liveAt, receivedLive),
        );
      }
    }
    final perRoom = <String, int>{};
    for (final row in newRows) {
      perRoom[row.room] = (perRoom[row.room] ?? 0) + 1;
    }
    final skips = userStreamSkipVerification([
      for (final room in predicted)
        (completed[room] ?? false, perRoom[room] ?? 0),
    ]);
    result
      ..add('verifiedSkips', skips.verified)
      ..add('falseSkip', skips.falseSkip);
    final verdicts = userStreamAuditVerdicts({
      for (final room in sampled) room: completed[room] ?? false,
    }, perRoom);
    result
      ..add('skipAudited', verdicts.audited)
      ..add('skipAuditNew', verdicts.auditNew)
      ..add('skipAuditFailed', verdicts.failed);
    if (!job.fallback) {
      final missed = userStreamLiveMissed(
        prev: job.prev,
        completed: {
          for (final MapEntry(:key, :value) in completed.entries)
            if (value) key,
        },
        newRows: newRows,
        relayPubkey: job.relayPubkey,
      );
      result
        ..add('liveRoomsChecked', missed.checked)
        ..add('liveMissed', missed.missed);
    }
    return result;
  }
}
