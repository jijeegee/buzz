/// Newest windows a `/query` asks for, and their answers, as head spans of
/// the device message store. Mirrors desktop's `relayStoreHeads.ts`. Rules:
/// docs/message-sync.md#head-spans.
library;

import 'dart:convert';

import 'nostr_models.dart';

/// Kind of the relay-signed overlay that closes every thread window.
const _threadWindowBounds = 39007;

/// Rows of the window a room or thread opens on; narrower answers record
/// nothing, so they never stand in for the open window.
const storeHeadMinRows = 50;

/// A newest room or thread window one filter asks for.
typedef StoreHeadWindow = ({String roomId, String? rootId, List<int> kinds});

/// One newest window as the relay answered it.
typedef StoreHeadAnswer = ({
  StoreHeadWindow window,
  bool hasMore,
  (int, String)? nextCursor,
  Set<String> ids,
});

String? _single(List<String>? values) =>
    values != null && values.length == 1 ? values.single : null;

/// The newest window [filter] asks for: a room window (`top_level`) or a
/// thread window (`thread_window`) with no cursor, at least
/// [storeHeadMinRows] rows; else `null`.
StoreHeadWindow? storeHeadWindowOf(NostrFilter filter) {
  final extensions = filter.extensions;
  if (filter.limit < storeHeadMinRows ||
      filter.until != null ||
      filter.since != null ||
      extensions.containsKey('before_id') ||
      extensions.containsKey('after_id')) {
    return null;
  }
  final roomId = _single(filter.tags['#h']);
  if (roomId == null) return null;
  if (extensions['top_level'] == true) {
    return (roomId: roomId, rootId: null, kinds: filter.kinds);
  }
  final rootId = _single(filter.tags['#e']);
  if (extensions['thread_window'] == true && rootId != null) {
    return (roomId: roomId, rootId: rootId, kinds: filter.kinds);
  }
  return null;
}

/// [filter], pinned to the relay's writer when it asks for a newest room
/// window: its answer may drop stored rows it lacks. The thread-window
/// parser rejects unknown fields, so thread filters are left as they are.
NostrFilter storeHeadFilter(NostrFilter filter) {
  final window = storeHeadWindowOf(filter);
  if (window == null || window.rootId != null) return filter;
  return NostrFilter(
    kinds: filter.kinds,
    authors: filter.authors,
    ids: filter.ids,
    limit: filter.limit,
    since: filter.since,
    until: filter.until,
    search: filter.search,
    tags: filter.tags,
    extensions: {...filter.extensions, 'consistency': 'strong'},
  );
}

String _windowKey(StoreHeadWindow window) =>
    '${window.roomId}\u0000${window.rootId ?? ''}';

/// The newest windows of [windows] read out of a `/query` answer: each run
/// of events up to the bounds overlay that closes it (`39006` keyed
/// `<room>:head`, `39007` tagged with its room and root, paging `older`).
/// Bounds are matched by room and root only, so a window asked twice, or
/// answered twice, records nothing.
List<StoreHeadAnswer> storeHeadAnswers(
  List<StoreHeadWindow> asked,
  List<NostrEvent> events,
) {
  final counts = <String, int>{};
  for (final window in asked) {
    counts.update(_windowKey(window), (n) => n + 1, ifAbsent: () => 1);
  }
  final windows = [
    for (final window in asked)
      if (counts[_windowKey(window)] == 1) window,
  ];
  final answers = <StoreHeadAnswer>[];
  var segment = <String>{};
  for (final event in events) {
    final bounds =
        event.kind == EventKind.channelWindowBounds ||
        event.kind == _threadWindowBounds;
    if (!bounds) {
      segment.add(event.id);
      continue;
    }
    final ids = segment;
    segment = {};
    final roomId = event.getTagValue('h');
    final StoreHeadWindow? window;
    if (event.kind == EventKind.channelWindowBounds) {
      window = windows
          .where(
            (window) =>
                window.rootId == null &&
                event.getTagValue('d') == '${window.roomId.toLowerCase()}:head',
          )
          .firstOrNull;
    } else {
      final rootId = event.getTagValue('e');
      window = windows
          .where(
            (window) =>
                window.rootId != null &&
                window.roomId == roomId &&
                window.rootId == rootId,
          )
          .firstOrNull;
    }
    if (window == null) continue;
    final Object? content;
    try {
      content = jsonDecode(event.content);
    } on FormatException {
      continue;
    }
    if (content is! Map || content['has_more'] is! bool) continue;
    if (window.rootId != null && content['direction'] != 'older') continue;
    final hasMore = content['has_more'] as bool;
    final next = content['next_cursor'];
    final nextCursor =
        next is Map && next['created_at'] is int && next['id'] is String
        ? (next['created_at'] as int, next['id'] as String)
        : null;
    if (hasMore && nextCursor == null) continue;
    answers.add((
      window: window,
      hasMore: hasMore,
      nextCursor: hasMore ? nextCursor : null,
      ids: ids,
    ));
  }
  final answered = <String, int>{};
  for (final answer in answers) {
    answered.update(_windowKey(answer.window), (n) => n + 1, ifAbsent: () => 1);
  }
  return [
    for (final answer in answers)
      if (answered[_windowKey(answer.window)] == 1) answer,
  ];
}
