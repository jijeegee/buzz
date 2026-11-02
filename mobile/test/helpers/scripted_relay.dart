import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;
import 'package:kuzz/shared/relay/relay.dart';

/// One frame on the scripted socket, in the order it was seen.
///
/// [type] is `REQ` or `CLOSE` (client to relay) or `CLOSED` (relay to
/// client). [filters] holds every filter of a REQ (`message.sublist(2)`), so
/// a multi-filter REQ is described in full.
class RelayFrame {
  RelayFrame(this.type, this.subId, [this.filters = const []]);

  final String type;
  final String subId;
  final List<Map<String, dynamic>> filters;

  @override
  String toString() => '$type $subId ${jsonEncode(filters)}';
}

/// An in-memory relay behind the real [RelaySessionNotifier].
///
/// It answers websocket REQs (live `l-` and one-shot `h-`) and HTTP `/query`
/// from one event list, with a naive NIP-01 filter matcher plus the NIP-CW
/// channel and thread windows the app pages through. Every REQ, CLOSE and
/// relay CLOSED is logged; [openSubs] is derived from that log only, never
/// from the session's own bookkeeping.
class ScriptedRelay {
  ScriptedRelay({required this.relayKey});

  /// The relay's own signing pubkey (NIP-11 `self`).
  final String relayKey;

  final List<NostrEvent> events = [];
  final List<RelayFrame> frames = [];
  final List<List<Map<String, dynamic>>> httpQueries = [];

  /// Open subscriptions by subId, in REQ order: a REQ adds or replaces, a
  /// client CLOSE or relay CLOSED removes.
  final Map<String, List<Map<String, dynamic>>> openSubs = {};

  /// Sockets the session created. More than one means it reconnected.
  int connects = 0;

  /// When set, live REQs whose subId it accepts are answered with CLOSED.
  String? Function(List<Map<String, dynamic>> filters)? closeLive;

  /// The answer to a user stream REQ (`#h: [$joined]`): by default the
  /// CLOSED of a relay without the stream. Set to `null` to answer with
  /// [userStreamSnapshot] (part 1's content), then EOSE.
  String? userStreamClosed = 'restricted: not a channel member';
  Map<String, dynamic>? userStreamSnapshot;

  /// Live REQ frames sent so far (re-REQs included).
  int get liveReqSends =>
      frames.where((f) => f.type == 'REQ' && f.subId.startsWith('l-')).length;

  /// One-shot websocket REQ frames sent so far.
  int get oneShotReqSends =>
      frames.where((f) => f.type == 'REQ' && f.subId.startsWith('h-')).length;

  /// Open live subscriptions (`l-`).
  Map<String, List<Map<String, dynamic>>> get openLive => {
    for (final entry in openSubs.entries)
      if (entry.key.startsWith('l-')) entry.key: entry.value,
  };

  /// Open subscriptions that are not live: one-shots that were never closed
  /// and any id outside the known `l-`/`h-` scheme.
  Map<String, List<Map<String, dynamic>>> get openUnclassified => {
    for (final entry in openSubs.entries)
      if (!entry.key.startsWith('l-')) entry.key: entry.value,
  };

  /// Sorted, normalised signatures of the open live subscriptions.
  List<String> liveSignatures() =>
      [for (final filters in openLive.values) reqSignature(filters)]..sort();

  /// Builds a socket for [RelaySessionNotifier.new]'s `socketFactory`.
  RelaySocket socketFactory({
    required String wsUrl,
    required String? nsec,
    required void Function(List<dynamic> message) onMessage,
    required void Function() onConnected,
    required void Function(Object? error) onDisconnected,
  }) {
    // A new socket starts with no subscriptions (the old one's are gone).
    if (connects > 0) {
      openSubs.clear();
      _userStreamSub = null;
    }
    connects++;
    _dropSocket = onDisconnected;
    return _ScriptedRelaySocket(
      relay: this,
      wsUrl: wsUrl,
      deliver: onMessage,
      connected: onConnected,
    );
  }

  /// HTTP client for the session's `/query` bridge.
  late final http.Client httpClient = http_testing.MockClient((request) async {
    if (request.method != 'POST' || request.url.path != '/query') {
      return http.Response('not found', 404);
    }
    final filters = [
      for (final filter in jsonDecode(request.body) as List)
        (filter as Map).cast<String, dynamic>(),
    ];
    httpQueries.add(filters);
    return http.Response(
      jsonEncode([
        for (final filter in filters)
          for (final event in answer(filter)) event.toJson(),
      ]),
      200,
      headers: {'content-type': 'application/json'},
    );
  });

  void _onClientFrame(List<dynamic> message, void Function(List) deliver) {
    final type = message.first;
    if (type == 'EVENT') {
      final event = NostrEvent.fromJson(message[1] as Map<String, dynamic>);
      scheduleMicrotask(() => deliver(['OK', event.id, true, '']));
      return;
    }
    if (type == 'CLOSE') {
      final subId = message[1] as String;
      frames.add(RelayFrame('CLOSE', subId));
      openSubs.remove(subId);
      return;
    }
    if (type != 'REQ') return;
    final subId = message[1] as String;
    final filters = [
      for (final filter in message.sublist(2))
        Map<String, dynamic>.from(
          jsonDecode(jsonEncode(filter)) as Map<String, dynamic>,
        ),
    ];
    frames.add(RelayFrame('REQ', subId, filters));
    openSubs.remove(subId);
    openSubs[subId] = filters;
    if ((filters.first['#h'] as List?)?.contains(r'$joined') ?? false) {
      _answerUserStream(subId, deliver);
      return;
    }
    final rejection = subId.startsWith('l-') ? closeLive?.call(filters) : null;
    scheduleMicrotask(() {
      if (rejection != null) {
        frames.add(RelayFrame('CLOSED', subId));
        openSubs.remove(subId);
        deliver(['CLOSED', subId, rejection]);
        return;
      }
      for (final filter in filters) {
        for (final event in answer(filter)) {
          deliver(['EVENT', subId, event.toJson()]);
        }
      }
      deliver(['EOSE', subId]);
    });
  }

  void Function(Object? error)? _dropSocket;

  /// The relay drops the current socket: the session hears it end and
  /// reconnects on its own schedule.
  void dropSocket() {
    final drop = _dropSocket;
    _dropSocket = null;
    openSubs.clear();
    _userStreamSub = null;
    drop?.call(null);
  }

  /// Ends the open stream sub with a relay CLOSED [reason].
  void closeUserStream(String reason) {
    final open = _userStreamSub;
    if (open == null) return;
    _userStreamSub = null;
    frames.add(RelayFrame('CLOSED', open.$1));
    openSubs.remove(open.$1);
    open.$2(['CLOSED', open.$1, reason]);
  }

  (String, void Function(List))? _userStreamSub;

  void _answerUserStream(String subId, void Function(List) deliver) {
    _userStreamSub = (subId, deliver);
    final closed = userStreamClosed;
    final snapshot = userStreamSnapshot;
    scheduleMicrotask(() {
      if (closed != null || snapshot == null) {
        frames.add(RelayFrame('CLOSED', subId));
        openSubs.remove(subId);
        deliver([
          'CLOSED',
          subId,
          closed ?? 'restricted: not a channel member',
        ]);
        return;
      }
      deliver([
        'EVENT',
        subId,
        {
          'id': 'snapshot-$subId',
          'pubkey': relayKey,
          'created_at': DateTime.now().millisecondsSinceEpoch ~/ 1000,
          'kind': 24220,
          'tags': const [],
          'content': jsonEncode(snapshot),
          'sig': '',
        },
      ]);
      deliver(['EOSE', subId]);
    });
  }

  /// Events matching [filter], newest first, as the relay would serve them.
  List<NostrEvent> answer(Map<String, dynamic> filter) {
    if (filter['top_level'] == true) return _channelWindow(filter);
    if (filter['thread_window'] == true) return _threadWindow(filter);
    final matched = events.where((e) => _matches(e, filter)).toList()
      ..sort(_relayOrder);
    final limit = filter['limit'] as int?;
    return limit == null ? matched : matched.take(limit).toList();
  }

  List<NostrEvent> _channelWindow(Map<String, dynamic> filter) {
    final channelId = (filter['#h'] as List).single as String;
    final rows =
        events
            .where(
              (e) =>
                  _matches(e, filter, keyset: true) &&
                  e.threadReference.rootId == null,
            )
            .toList()
          ..sort(_relayOrder);
    final limit = filter['limit'] as int;
    final page = rows.take(limit).toList();
    final hasMore = rows.length > limit;
    final newer = filter['after_id'] != null;
    final suffix = newer
        ? 'after:${filter['since']}:${filter['after_id']}'
        : filter['before_id'] != null
        ? '${filter['until']}:${filter['before_id']}'
        : 'head';
    return [
      ...page,
      for (final row in page) ?_threadSummary(channelId, row),
      NostrEvent(
        id: 'bounds-$suffix',
        pubkey: relayKey,
        createdAt: 0,
        kind: EventKind.channelWindowBounds,
        tags: [
          ['d', '${channelId.toLowerCase()}:$suffix'],
          ['h', channelId],
        ],
        content: jsonEncode({
          if (newer) 'direction': 'newer',
          'has_more': hasMore,
          'next_cursor': hasMore
              ? {'created_at': page.last.createdAt, 'id': page.last.id}
              : null,
        }),
        sig: '',
      ),
    ];
  }

  NostrEvent? _threadSummary(String channelId, NostrEvent root) {
    final replies = _repliesTo(root.id);
    if (replies.isEmpty) return null;
    final last = replies
        .map((e) => e.createdAt)
        .reduce((a, b) => a > b ? a : b);
    return NostrEvent(
      id: 'summary-${root.id}',
      pubkey: relayKey,
      createdAt: last,
      kind: EventKind.channelThreadSummary,
      tags: [
        ['h', channelId],
        ['e', root.id],
      ],
      content: jsonEncode({
        'reply_count': replies.length,
        'descendant_count': replies.length,
        'last_reply_at': last,
        'participants': {for (final r in replies) r.pubkey}.toList(),
      }),
      sig: '',
    );
  }

  List<NostrEvent> _repliesTo(String rootId) => [
    for (final e in events)
      if (e.threadReference.rootId == rootId &&
          EventKind.channelMessageEventKinds.contains(e.kind))
        e,
  ];

  List<NostrEvent> _threadWindow(Map<String, dynamic> filter) {
    final channelId = (filter['#h'] as List).single as String;
    final rootId = (filter['#e'] as List).single as String;
    final rows =
        _repliesTo(rootId)
            .where((e) => _matches(e, {...filter}..remove('#e'), keyset: true))
            .toList()
          ..sort(_relayOrder);
    final limit = filter['limit'] as int;
    final page = rows.take(limit).toList();
    final hasMore = rows.length > limit;
    return [
      ...page,
      NostrEvent(
        id: 'thread-bounds-$rootId',
        pubkey: relayKey,
        createdAt: 0,
        kind: 39007,
        tags: [
          ['h', channelId],
          ['e', rootId],
        ],
        content: jsonEncode({
          if (filter['after_id'] != null) 'direction': 'newer',
          'has_more': hasMore,
          'next_cursor': hasMore
              ? {'created_at': page.last.createdAt, 'id': page.last.id}
              : null,
        }),
        sig: '',
      ),
    ];
  }

  static int _relayOrder(NostrEvent a, NostrEvent b) =>
      a.createdAt != b.createdAt
      ? b.createdAt.compareTo(a.createdAt)
      : a.id.compareTo(b.id);

  static bool _matches(
    NostrEvent event,
    Map<String, dynamic> filter, {
    bool keyset = true,
  }) {
    final kinds = filter['kinds'] as List?;
    if (kinds != null && !kinds.contains(event.kind)) return false;
    final ids = filter['ids'] as List?;
    if (ids != null && !ids.contains(event.id)) return false;
    final authors = filter['authors'] as List?;
    if (authors != null && !authors.contains(event.pubkey)) return false;
    for (final entry in filter.entries) {
      if (!entry.key.startsWith('#') || entry.key.length != 2) continue;
      final name = entry.key.substring(1);
      final wanted = (entry.value as List).cast<Object?>();
      if (!event.tags.any(
        (t) => t.length >= 2 && t[0] == name && wanted.contains(t[1]),
      )) {
        return false;
      }
    }
    final since = filter['since'] as int?;
    final until = filter['until'] as int?;
    final afterId = filter['after_id'] as String?;
    final beforeId = filter['before_id'] as String?;
    if (keyset && afterId != null && since != null) {
      if (!(event.createdAt > since ||
          (event.createdAt == since && event.id.compareTo(afterId) < 0))) {
        return false;
      }
    } else if (since != null && event.createdAt < since) {
      return false;
    }
    if (keyset && beforeId != null && until != null) {
      if (!(event.createdAt < until ||
          (event.createdAt == until && event.id.compareTo(beforeId) > 0))) {
        return false;
      }
    } else if (until != null && event.createdAt > until) {
      return false;
    }
    return true;
  }
}

/// The normalised shape of one REQ: each filter's keys in sorted order, with
/// ids, pubkeys and room ids reduced to a count, timestamps to their key and
/// kinds sorted, so a signature names what a subscription asks for, not whose
/// data it is. The presence author set grows with what the screen shows, so
/// its size is `authors:+`. Filters of a multi-filter REQ are joined with
/// ` + ` in sorted order. Same rules as desktop's `e2eBridgeReqLog.ts`.
String reqSignature(List<Map<String, dynamic>> filters) =>
    (filters.map(filterSignature).toList()..sort()).join(' + ');

String filterSignature(Map<String, dynamic> filter) {
  final keys = filter.keys.toList()..sort();
  final presence =
      filter['kinds'] is List &&
      (filter['kinds'] as List).length == 1 &&
      (filter['kinds'] as List).single == EventKind.presenceUpdate;
  return [
    for (final key in keys)
      if (key == 'since' || key == 'until' || key == 'search')
        key
      else if (key == 'authors' && presence)
        'authors:+'
      else
        '$key:${_signatureValue(key, filter[key])}',
  ].join('|');
}

final _idLike = RegExp(
  r'^([0-9a-f]{64}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$',
  caseSensitive: false,
);

String _signatureValue(String key, Object? value) {
  if (value is List) {
    if (key == 'kinds') {
      return '[${([for (final v in value) v as int]..sort()).join(',')}]';
    }
    if (value.every((v) => v is String && _idLike.hasMatch(v))) {
      return '${value.length}';
    }
    return '[${[for (final v in value) v is String && _idLike.hasMatch(v) ? '*' : v].join(',')}]';
  }
  if (value is String && _idLike.hasMatch(value)) return '*';
  return '$value';
}

class _ScriptedRelaySocket extends RelaySocket {
  _ScriptedRelaySocket({
    required this.relay,
    required super.wsUrl,
    required this.deliver,
    required this.connected,
  }) : super(
         nsec: null,
         onMessage: deliver,
         onConnected: connected,
         onDisconnected: (_) {},
       );

  final ScriptedRelay relay;
  final void Function(List<dynamic> message) deliver;
  final void Function() connected;

  @override
  Future<void> connect() async {
    scheduleMicrotask(connected);
  }

  @override
  void send(List<dynamic> payload) => relay._onClientFrame(payload, deliver);

  @override
  Future<void> disconnect() async {}

  @override
  void dispose() {}
}
