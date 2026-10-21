import 'dart:convert';
import 'dart:math';

import 'nostr_models.dart';

/// Channel order numbers (`channel_sequence`) from a relay whose NIP-11
/// `supported_extensions` lists [kuzzSeqExtension]. The socket opts in by
/// connecting with `?kuzz_seq=1`; a relay without it is treated as today.
const kuzzSeqExtension = 'kuzz-seq';

/// Page size for `after_seq` repair (the relay caps it at 500).
const afterSeqPageLimit = 500;

/// Pages one repair reads per channel. Anything past them is left to the
/// screens' own refresh.
const afterSeqMaxPages = 10;

/// How long the newest number must have been the newest before repair may
/// start from it.
const seqStableAfter = Duration(seconds: 30);

/// The `channel_sequence` in the trailing object of an EVENT (index 3) or OK
/// (index 4) frame, or `null`.
int? frameChannelSequence(List<dynamic> frame, int index) {
  if (frame.length <= index) return null;
  final extra = frame[index];
  if (extra is! Map) return null;
  final sequence = extra['channel_sequence'];
  return sequence is int && sequence >= 0 ? sequence : null;
}

/// The facts of the `kind:39006` event that closes an `after_seq` page.
class AfterSeqBounds {
  final int lastSeq;
  final bool hasMore;

  /// Event id to order number, for every event in the page.
  final Map<String, int> sequences;

  const AfterSeqBounds({
    required this.lastSeq,
    required this.hasMore,
    this.sequences = const {},
  });
}

/// Whether [event] closes an `after_seq` page. Channel-window bounds share
/// kind 39006; these carry `order: "seq"` and a `<channel>:after_seq:<N>` `d`.
bool isAfterSeqBounds(NostrEvent event) =>
    event.kind == EventKind.channelWindowBounds &&
    ((event.getTagValue('d')?.contains(':after_seq:') ?? false) ||
        parseAfterSeqBounds(event) != null);

/// [AfterSeqBounds] when [event] closes an `after_seq` page, else `null`.
AfterSeqBounds? parseAfterSeqBounds(NostrEvent event) {
  if (event.kind != EventKind.channelWindowBounds) return null;
  final Object? content;
  try {
    content = jsonDecode(event.content);
  } on FormatException {
    return null;
  }
  if (content is! Map || content['order'] != 'seq') return null;
  final lastSeq = content['last_seq'];
  if (lastSeq is! int) return null;
  final sequences = <String, int>{};
  final pairs = content['sequences'];
  if (pairs is List) {
    for (final pair in pairs) {
      if (pair is List && pair.length == 2) {
        final id = pair[0];
        final sequence = pair[1];
        if (id is String && sequence is int) sequences[id] = sequence;
      }
    }
  }
  return AfterSeqBounds(
    lastSeq: lastSeq,
    hasMore: content['has_more'] == true,
    sequences: sequences,
  );
}

/// The order numbers seen live in one channel, and where repair starts.
///
/// Live numbers are not dense for one reader (kind filters skip some), so a
/// jump is no proof of a gap, and a gap can arrive before the relay's resync
/// signal. Repair therefore starts from a number that was already the newest
/// [seqStableAfter] ago, or just before the first number seen when the channel
/// has not been watched that long. Events seen twice are deduplicated by id.
/// Mirrors kuzz-acp's `SeqCursor`.
class ChannelSeqCursor {
  ChannelSeqCursor(int sequence, this.lastObservedAt)
    : _first = sequence,
      _newest = sequence;

  final int _first;
  int _newest;
  int? _stable;
  int? _candidate;
  DateTime? _candidateAt;

  /// When the channel last delivered a number; repair goes to the most
  /// recently active channels first.
  DateTime lastObservedAt;

  int get newest => _newest;

  /// Record a number delivered at [now].
  void observe(int sequence, DateTime now) {
    lastObservedAt = now;
    _promote(now);
    if (_candidate == null) {
      _candidate = _newest;
      _candidateAt = now;
    }
    _newest = max(_newest, sequence);
  }

  /// The `after_seq` a repair at [now] starts from.
  int repairPoint(DateTime now) {
    _promote(now);
    return _stable ?? max(0, _first - 1);
  }

  void _promote(DateTime now) {
    final candidate = _candidate;
    final at = _candidateAt;
    if (candidate == null || at == null) return;
    if (now.difference(at) < seqStableAfter) return;
    _stable = max(_stable ?? candidate, candidate);
    _candidate = null;
    _candidateAt = null;
  }
}

/// Kind:40902 presence snapshot, which the relay builds and never stores.
const _presenceSnapshotKind = 40902;

/// Kinds a repair may ask `after_seq` for: stored kinds only, since the relay
/// refuses presence and ephemeral kinds have no order number.
List<int> afterSeqKinds(Iterable<int> kinds) => [
  for (final kind in kinds.toSet())
    if ((kind < 20000 || kind >= 30000) && kind != _presenceSnapshotKind) kind,
];

/// Whether [event] matches a live subscription's NIP-01 [filter]: ids,
/// authors, kinds, tag filters and the time window. Kuzz filter extensions
/// are not checked; they only shape history replies.
bool liveFilterMatches(NostrFilter filter, NostrEvent event) {
  if (filter.ids case final ids? when !ids.contains(event.id)) return false;
  if (filter.authors case final authors? when !authors.contains(event.pubkey)) {
    return false;
  }
  if (!filter.kinds.contains(event.kind)) return false;
  if (filter.since case final since? when event.createdAt < since) return false;
  if (filter.until case final until? when event.createdAt > until) return false;
  for (final MapEntry(key: key, value: values) in filter.tags.entries) {
    if (!key.startsWith('#') || key.length != 2) continue;
    final name = key.substring(1);
    final matched = event.tags.any(
      (tag) => tag.length > 1 && tag[0] == name && values.contains(tag[1]),
    );
    if (!matched) return false;
  }
  return true;
}
