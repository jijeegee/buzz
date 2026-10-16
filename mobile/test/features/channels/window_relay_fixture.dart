import 'dart:convert';

import 'package:buzz/shared/relay/relay.dart';

/// A 64-hex event id that sorts by [n].
String fixtureId(int n) => n.toRadixString(16).padLeft(64, '0');

NostrEvent fixtureRow(
  String channelId,
  int n,
  int createdAt, {
  List<List<String>> extraTags = const [],
}) => NostrEvent(
  id: fixtureId(n),
  pubkey: 'a' * 64,
  createdAt: createdAt,
  kind: EventKind.streamMessageV2,
  tags: [
    ['h', channelId],
    ...extraTags,
  ],
  content: 'row $n',
  sig: '',
);

/// The relay's window order: newest first, ascending id within a second.
int compareRelayOrder(NostrEvent left, NostrEvent right) {
  if (left.createdAt != right.createdAt) {
    return right.createdAt - left.createdAt;
  }
  return left.id.compareTo(right.id);
}

/// Serves one NIP-CW channel window over [rows] the way the relay's keyset
/// SQL does: a head page, an older page (`until` + `before_id`), or a newer
/// page (`since` + `after_id`, mirrored order), closed by its bounds overlay.
List<NostrEvent> serveChannelWindow(
  String channelId,
  Iterable<NostrEvent> rows,
  NostrFilter filter,
) {
  final afterId = filter.extensions['after_id'] as String?;
  final beforeId = filter.extensions['before_id'] as String?;
  final newer = afterId != null;
  final List<NostrEvent> candidates;
  final String suffix;
  if (newer) {
    final since = filter.since!;
    candidates =
        rows
            .where(
              (row) =>
                  row.createdAt > since ||
                  (row.createdAt == since && row.id.compareTo(afterId) < 0),
            )
            .toList()
          ..sort((a, b) => compareRelayOrder(b, a));
    suffix = 'after:$since:$afterId';
  } else if (beforeId != null) {
    final until = filter.until!;
    candidates =
        rows
            .where(
              (row) =>
                  row.createdAt < until ||
                  (row.createdAt == until && row.id.compareTo(beforeId) > 0),
            )
            .toList()
          ..sort(compareRelayOrder);
    suffix = '$until:$beforeId';
  } else {
    candidates = rows.toList()..sort(compareRelayOrder);
    suffix = 'head';
  }
  final page = candidates.take(filter.limit).toList();
  final hasMore = candidates.length > filter.limit;
  return [
    ...page,
    NostrEvent(
      id: 'bounds-$suffix',
      pubkey: 'relay',
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
