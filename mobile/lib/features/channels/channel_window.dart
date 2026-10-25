import 'dart:convert';

import '../../shared/relay/relay.dart';
import 'channel_event_order.dart';
import 'window_anchor.dart';

/// One NIP-CW window page of [channelId]: the newest without a [cursor];
/// older pages run back from it (`until` + `before_id`), newer pages forward
/// from it (`since` + `after_id`).
NostrFilter channelWindowFilter(
  String channelId,
  ChannelPageCursor? cursor, {
  WindowPageDirection direction = WindowPageDirection.older,
  int limit = 50,
}) {
  final newer = direction == WindowPageDirection.newer;
  return NostrFilter(
    kinds: EventKind.channelTimelineContentKinds,
    tags: {
      '#h': [channelId],
    },
    limit: limit,
    until: newer ? null : cursor?.createdAt,
    since: newer ? cursor?.createdAt : null,
    extensions: {
      'top_level': true,
      'include_summaries': true,
      'include_aux': true,
      if (cursor != null) newer ? 'after_id' : 'before_id': cursor.eventId,
    },
  );
}

class ChannelPageCursor {
  final int createdAt;
  final String eventId;

  const ChannelPageCursor({required this.createdAt, required this.eventId});

  factory ChannelPageCursor.of(NostrEvent event) =>
      ChannelPageCursor(createdAt: event.createdAt, eventId: event.id);
}

class ChannelWindowThreadSummary {
  final int replyCount;
  final int descendantCount;
  final int? lastReplyAt;
  final List<String> participantPubkeys;

  /// True while capped evidence awaits a complete recount.
  final bool isLowerBound;

  /// The last known total is awaiting reconciliation and should not be displayed.
  final bool isCountPending;

  const ChannelWindowThreadSummary({
    required this.replyCount,
    required this.descendantCount,
    required this.lastReplyAt,
    required this.participantPubkeys,
    this.isLowerBound = false,
    this.isCountPending = false,
  });
}

/// A thread-summary snapshot received from the live channel subscription.
///
/// [createdAt] is the relay event timestamp used to reject delayed snapshots
/// after a fresher recount has already been displayed. Recounts created in the
/// same second retain their live delivery order, because relay timestamps have
/// second precision.
class ChannelWindowLiveThreadSummary {
  final ChannelWindowThreadSummary summary;
  final int createdAt;

  const ChannelWindowLiveThreadSummary({
    required this.summary,
    required this.createdAt,
  });
}

class ChannelWindowRow {
  final NostrEvent event;
  final ChannelWindowThreadSummary? thread;
  final int? threadSummaryCreatedAt;

  const ChannelWindowRow({
    required this.event,
    this.thread,
    this.threadSummaryCreatedAt,
  });

  ChannelWindowRow copyWith({
    ChannelWindowThreadSummary? thread,
    int? threadSummaryCreatedAt,
  }) => ChannelWindowRow(
    event: event,
    thread: thread ?? this.thread,
    threadSummaryCreatedAt:
        threadSummaryCreatedAt ?? this.threadSummaryCreatedAt,
  );
}

class ChannelWindowPage {
  final ChannelPageCursor? startCursor;

  /// Rows in this page's own relay order: newest first for an older page,
  /// oldest first for a newer one.
  final List<ChannelWindowRow> rows;
  final List<NostrEvent> aux;
  final ChannelPageCursor? nextCursor;
  final bool hasMore;
  final WindowPageDirection direction;

  const ChannelWindowPage({
    required this.startCursor,
    required this.rows,
    required this.aux,
    required this.nextCursor,
    required this.hasMore,
    this.direction = WindowPageDirection.older,
  });
}

class ChannelWindowStore {
  /// Older pages chained cursor to cursor, newest first. The first one starts
  /// at the newest row, or at [anchorCursor] for a window opened at a message.
  final List<ChannelWindowPage> pages;

  /// Newer pages chained from [anchorCursor] toward the present; empty for a
  /// window opened at the newest row.
  final List<ChannelWindowPage> newerPages;

  /// Where a window opened at a message (or read marker) splits into older
  /// and newer pages; null for a window opened at the newest row.
  final ChannelPageCursor? anchorCursor;

  /// The message the window was opened at, when there was one.
  final String? anchorId;

  /// Whether rows newer than every loaded row still have to be fetched from
  /// [newerCursor] before this window reaches the present.
  final bool hasNewer;
  final ChannelPageCursor? newerCursor;

  /// Live main-timeline rows that arrived while [hasNewer]. They belong after
  /// rows not loaded yet, so they wait here instead of showing past a gap, and
  /// join [liveOverlay] once the newer pages reach the present.
  final List<NostrEvent> heldLive;
  final List<NostrEvent> liveOverlay;
  final List<NostrEvent> liveAux;

  /// Thread summaries that arrived over the live socket, keyed by root event id.
  ///
  /// Kept beside the pages rather than folded into [ChannelWindowRow.thread]
  /// because a root can be in [liveOverlay] instead — a message you just sent
  /// has no row yet, and its reply count has to land somewhere.
  final Map<String, ChannelWindowLiveThreadSummary> liveThreadSummaries;

  const ChannelWindowStore({
    required this.pages,
    required this.liveThreadSummaries,
    required this.liveOverlay,
    required this.liveAux,
    this.newerPages = const [],
    this.anchorCursor,
    this.anchorId,
    this.hasNewer = false,
    this.newerCursor,
    this.heldLive = const [],
  });

  const ChannelWindowStore.empty()
    : pages = const [],
      newerPages = const [],
      anchorCursor = null,
      anchorId = null,
      hasNewer = false,
      newerCursor = null,
      heldLive = const [],
      liveOverlay = const [],
      liveAux = const [],
      liveThreadSummaries = const {};

  /// Whether this window was opened at a message rather than the newest row.
  bool get isMiddle => anchorCursor != null;

  /// Every page, older and newer, for lookups that ignore order.
  Iterable<ChannelWindowPage> get allPages => [...pages, ...newerPages];

  /// A copy with the given parts replaced and the window shape kept.
  ChannelWindowStore copyWith({
    List<ChannelWindowPage>? pages,
    List<NostrEvent>? liveOverlay,
    List<NostrEvent>? liveAux,
    Map<String, ChannelWindowLiveThreadSummary>? liveThreadSummaries,
    List<NostrEvent>? heldLive,
  }) => ChannelWindowStore(
    pages: pages ?? this.pages,
    liveOverlay: liveOverlay ?? this.liveOverlay,
    liveAux: liveAux ?? this.liveAux,
    liveThreadSummaries: liveThreadSummaries ?? this.liveThreadSummaries,
    newerPages: newerPages,
    anchorCursor: anchorCursor,
    anchorId: anchorId,
    hasNewer: hasNewer,
    newerCursor: newerCursor,
    heldLive: heldLive ?? this.heldLive,
  );
}

ChannelWindowPage parseChannelWindowResponse(
  List<NostrEvent> events,
  String channelId,
  ChannelPageCursor? startCursor, {
  WindowPageDirection direction = WindowPageDirection.older,
}) {
  final newer = direction == WindowPageDirection.newer;
  if (newer && startCursor == null) {
    throw Exception('A newer channel page needs a cursor.');
  }
  var rows = [
    for (final event in events)
      if (EventKind.channelTimelineContentKinds.contains(event.kind))
        ChannelWindowRow(event: event),
  ];
  final rowIndexesById = <String, int>{
    for (var i = 0; i < rows.length; i++) rows[i].event.id: i,
  };

  for (final event in events) {
    if (event.kind != EventKind.channelThreadSummary) continue;
    final rootId = event.getTagValue('e');
    final rowIndex = rootId == null ? null : rowIndexesById[rootId];
    if (rowIndex == null) continue;
    rows[rowIndex] = rows[rowIndex].copyWith(
      thread: parseChannelWindowThreadSummary(event),
      threadSummaryCreatedAt: event.createdAt,
    );
  }

  final boundsEvents = events
      .where((event) => event.kind == EventKind.channelWindowBounds)
      .toList();
  if (boundsEvents.length != 1) {
    throw Exception(
      'Channel window response must contain exactly one bounds event.',
    );
  }
  final boundsEvent = boundsEvents.single;
  if (boundsEvent.getTagValue('d') !=
      _expectedBoundsKey(channelId, startCursor, newer: newer)) {
    throw Exception('Channel window bounds do not match the request cursor.');
  }

  final bounds = _parseJsonMap(boundsEvent, 'window bounds');
  // Older bounds predate the field and omit it; newer bounds must name it.
  final boundsDirection = bounds['direction'];
  if (newer ? boundsDirection != 'newer' : boundsDirection == 'newer') {
    throw Exception('Channel window bounds run the wrong direction.');
  }
  final hasMore = bounds['has_more'] as bool;
  final nextCursor = _parseCursor(bounds['next_cursor']);
  if (hasMore != (nextCursor != null)) {
    throw Exception('Channel window bounds has_more and next_cursor disagree.');
  }

  return ChannelWindowPage(
    startCursor: startCursor,
    rows: rows,
    aux: [
      for (final event in events)
        if (EventKind.channelAuxEventKinds.contains(event.kind)) event,
    ],
    nextCursor: nextCursor,
    hasMore: hasMore,
    direction: direction,
  );
}

/// Starts the window over at the newest page. A window opened at a message
/// ends here too: rows it was holding back are newer than this page's
/// snapshot or in it, so they join the overlay.
ChannelWindowStore replaceNewestChannelWindow(
  ChannelWindowStore current,
  ChannelWindowPage page, {
  Set<String> retainLiveSummaryRootIds = const {},
}) {
  if (page.startCursor != null) {
    throw Exception('Newest channel page must have a null start cursor.');
  }
  _assertValidPage(page);
  final rowIds = page.rows.map((row) => row.event.id).toSet();
  final auxIds = page.aux.map((event) => event.id).toSet();
  return ChannelWindowStore(
    pages: [page],
    liveOverlay: _sortedLive(
      [
        ...current.liveOverlay,
        ...current.heldLive,
      ].where((event) => !rowIds.contains(event.id)),
    ),
    liveAux: current.liveAux
        .where((event) => !auxIds.contains(event.id))
        .toList(),
    // A summary received while the page query was in flight can describe a
    // mutation after that query's snapshot. Retain just those summaries even
    // when the relay timestamp matches the page overlay; subscription replay
    // received before the query still compares timestamps normally.
    liveThreadSummaries: _retainLiveSummariesAfterPage(
      current,
      page.rows,
      retainLiveSummaryRootIds: retainLiveSummaryRootIds,
    ),
  );
}

Map<String, ChannelWindowLiveThreadSummary> _retainLiveSummariesAfterPage(
  ChannelWindowStore current,
  List<ChannelWindowRow> rows, {
  required Set<String> retainLiveSummaryRootIds,
}) {
  final rowsById = {for (final row in rows) row.event.id: row};
  return {
    for (final entry in current.liveThreadSummaries.entries)
      if (retainLiveSummaryRootIds.contains(entry.key) ||
          rowsById[entry.key] == null ||
          rowsById[entry.key]!.thread == null ||
          entry.value.createdAt >
              (rowsById[entry.key]!.threadSummaryCreatedAt ?? -1))
        entry.key: entry.value,
  };
}

ChannelWindowStore appendOlderChannelWindow(
  ChannelWindowStore current,
  ChannelWindowPage page,
) {
  _assertValidPage(page);
  if (page.direction != WindowPageDirection.older) {
    throw Exception('Only an older channel page extends the oldest end.');
  }
  if (current.pages.isEmpty) {
    throw Exception('Load the newest channel page first.');
  }
  final tail = current.pages.last;
  if (!tail.hasMore || tail.nextCursor == null) {
    throw Exception('The channel window is already complete.');
  }
  if (!_cursorsEqual(page.startCursor, tail.nextCursor)) {
    throw Exception(
      'Channel page does not continue the retained cursor chain.',
    );
  }
  _assertNoRetainedOverlap(current, page.rows);
  final pageIds = page.rows.map((row) => row.event.id).toSet();
  return current.copyWith(
    pages: [...current.pages, page],
    liveOverlay: current.liveOverlay
        .where((event) => !pageIds.contains(event.id))
        .toList(),
    // A live recount can arrive while this older page is in flight. Keep it
    // when it is fresher than the page's embedded snapshot, just as a newest
    // page replacement does.
    liveThreadSummaries: _retainLiveSummariesAfterPage(
      current,
      page.rows,
      retainLiveSummaryRootIds: const {},
    ),
  );
}

/// Opens the window at an anchor: [older] runs back from the anchor cursor
/// (the anchor message first, when there is one) and [newer] forward from the
/// same cursor, so together they hold every row around it exactly once. Live
/// rows are re-placed: those past an unloaded gap are held, the rest stay.
ChannelWindowStore openMiddleChannelWindow(
  ChannelWindowStore current,
  ChannelWindowPage older,
  ChannelWindowPage newer, {
  String? anchorId,
}) {
  if (older.direction != WindowPageDirection.older ||
      newer.direction != WindowPageDirection.newer) {
    throw Exception('A middle window needs an older and a newer page.');
  }
  final anchor = older.startCursor;
  if (anchor == null || !_cursorsEqual(anchor, newer.startCursor)) {
    throw Exception('Middle window pages must share one anchor cursor.');
  }
  _assertValidPage(older);
  _assertValidPage(newer);
  final rows = [...older.rows, ...newer.rows];
  final rowIds = <String>{};
  for (final row in rows) {
    if (!rowIds.add(row.event.id)) {
      throw Exception('Channel row ${row.event.id} is on both sides.');
    }
  }
  final auxIds = {
    for (final event in [...older.aux, ...newer.aux]) event.id,
  };
  return _placeLive(
    ChannelWindowStore(
      pages: [older],
      newerPages: [newer],
      anchorCursor: anchor,
      anchorId: anchorId,
      hasNewer: newer.hasMore,
      newerCursor: newer.nextCursor,
      liveOverlay: const [],
      liveAux: current.liveAux
          .where((event) => !auxIds.contains(event.id))
          .toList(),
      liveThreadSummaries: _retainLiveSummariesAfterPage(
        current,
        rows,
        retainLiveSummaryRootIds: const {},
      ),
    ),
    [
      ...current.liveOverlay,
      ...current.heldLive,
    ].where((event) => !rowIds.contains(event.id)),
  );
}

/// Extends a middle window toward the present with the page fetched from its
/// [ChannelWindowStore.newerCursor]. The page is authoritative for its span,
/// so live copies of its rows drop out; when it reaches the present, the held
/// live rows join the overlay.
ChannelWindowStore appendNewerChannelWindow(
  ChannelWindowStore current,
  ChannelWindowPage page,
) {
  _assertValidPage(page);
  if (page.direction != WindowPageDirection.newer) {
    throw Exception('Only a newer channel page extends the newest end.');
  }
  if (!current.hasNewer || current.newerCursor == null) {
    throw Exception('The channel window already reaches the present.');
  }
  if (!_cursorsEqual(page.startCursor, current.newerCursor)) {
    throw Exception('Channel page does not continue the newer cursor chain.');
  }
  _assertNoRetainedOverlap(current, page.rows);
  final pageIds = page.rows.map((row) => row.event.id).toSet();
  return _placeLive(
    ChannelWindowStore(
      pages: current.pages,
      newerPages: [...current.newerPages, page],
      anchorCursor: current.anchorCursor,
      anchorId: current.anchorId,
      hasNewer: page.hasMore,
      newerCursor: page.nextCursor,
      liveOverlay: const [],
      liveAux: current.liveAux,
      liveThreadSummaries: _retainLiveSummariesAfterPage(
        current,
        page.rows,
        retainLiveSummaryRootIds: const {},
      ),
    ),
    [
      ...current.liveOverlay,
      ...current.heldLive,
    ].where((event) => !pageIds.contains(event.id)),
  );
}

/// After a reconnect, a middle window cannot know what it missed past its
/// newest loaded row: page on from there instead of starting over at the
/// newest page, which would drop the rows the reader is looking at. Rows
/// already shown stay; live rows from now on wait for the gap to close.
ChannelWindowStore reopenNewerChannelWindowEdge(ChannelWindowStore current) {
  if (!current.isMiddle || current.hasNewer) return current;
  final newerRows = current.newerPages.expand((page) => page.rows);
  return ChannelWindowStore(
    pages: current.pages,
    newerPages: current.newerPages,
    anchorCursor: current.anchorCursor,
    anchorId: current.anchorId,
    hasNewer: true,
    // Newer pages run oldest first, so the newest loaded row is the last.
    newerCursor: newerRows.isEmpty
        ? current.anchorCursor
        : ChannelPageCursor.of(newerRows.last.event),
    heldLive: current.heldLive,
    liveOverlay: current.liveOverlay,
    liveAux: current.liveAux,
    liveThreadSummaries: current.liveThreadSummaries,
  );
}

/// Holds back the [arriving] rows newer than every row a window opened at
/// the newest row shows: the window becomes one opened at its newest shown
/// row, so they wait past a gap (behind the Latest button) instead of moving
/// the rows on screen. Kept as it is when none is newer, for a window opened
/// at a message, and for one with no rows.
ChannelWindowStore holdNewerChannelWindow(
  ChannelWindowStore current,
  Iterable<NostrEvent> arriving,
) {
  if (current.isMiddle || current.pages.isEmpty) return current;
  NostrEvent? newest;
  for (final event in [
    for (final page in current.pages)
      for (final row in page.rows) row.event,
    ...current.liveOverlay.where(_isMainTimelineRow),
  ]) {
    if (newest == null || _compareRelayOrder(event, newest) < 0) {
      newest = event;
    }
  }
  if (newest == null) return current;
  final shown = newest;
  if (!arriving.any(
    (event) =>
        EventKind.channelTimelineContentKinds.contains(event.kind) &&
        _isMainTimelineRow(event) &&
        _compareRelayOrder(event, shown) < 0,
  )) {
    return current;
  }
  final at = windowAnchorCursor(
    createdAt: newest.createdAt,
    eventId: newest.id,
  );
  final anchor = ChannelPageCursor(
    createdAt: at.createdAt,
    eventId: at.eventId,
  );
  final first = current.pages.first;
  return _placeLive(
    ChannelWindowStore(
      pages: [
        ChannelWindowPage(
          startCursor: anchor,
          rows: first.rows,
          aux: first.aux,
          nextCursor: first.nextCursor,
          hasMore: first.hasMore,
        ),
        ...current.pages.skip(1),
      ],
      anchorCursor: anchor,
      hasNewer: true,
      newerCursor: anchor,
      liveOverlay: const [],
      liveAux: current.liveAux,
      liveThreadSummaries: current.liveThreadSummaries,
    ),
    [...current.liveOverlay, ...current.heldLive],
  );
}

/// Sorts [events] into [store]: main-timeline rows past the unloaded gap are
/// held, everything else (rows inside the window, reply evidence) overlays.
ChannelWindowStore _placeLive(
  ChannelWindowStore store,
  Iterable<NostrEvent> events,
) {
  final held = <NostrEvent>[];
  final overlay = <NostrEvent>[];
  final seen = <String>{};
  for (final event in events) {
    if (!seen.add(event.id)) continue;
    (_holdsLive(store, event) ? held : overlay).add(event);
  }
  return ChannelWindowStore(
    pages: store.pages,
    newerPages: store.newerPages,
    anchorCursor: store.anchorCursor,
    anchorId: store.anchorId,
    hasNewer: store.hasNewer,
    newerCursor: store.newerCursor,
    heldLive: _sortedLive(held),
    liveOverlay: _sortedLive(overlay),
    liveAux: store.liveAux,
    liveThreadSummaries: store.liveThreadSummaries,
  );
}

bool _holdsLive(ChannelWindowStore store, NostrEvent event) {
  final edge = store.newerCursor;
  return store.hasNewer &&
      edge != null &&
      _isMainTimelineRow(event) &&
      _isStrictlyNewer(event, edge);
}

/// Top-level rows and broadcast replies; other replies are thread evidence.
bool _isMainTimelineRow(NostrEvent event) =>
    event.threadReference.parentId == null ||
    event.tags.any(
      (tag) => tag.length > 1 && tag[0] == 'broadcast' && tag[1] == '1',
    );

List<NostrEvent> _sortedLive(Iterable<NostrEvent> events) =>
    events.toList()..sort(_compareRelayOrder);

void _assertNoRetainedOverlap(
  ChannelWindowStore current,
  List<ChannelWindowRow> rows,
) {
  final retainedIds = {
    for (final page in current.allPages)
      for (final row in page.rows) row.event.id,
  };
  for (final row in rows) {
    if (retainedIds.contains(row.event.id)) {
      throw Exception('Channel row ${row.event.id} overlaps a retained page.');
    }
  }
}

/// Decode a kind-39005 thread summary. Same payload whether it came down as a
/// channel-window page overlay or over the live socket — one contract, two doors.
ChannelWindowThreadSummary parseChannelWindowThreadSummary(NostrEvent event) {
  final payload = _parseJsonMap(event, 'thread summary');
  final participants = payload['participants'];
  return ChannelWindowThreadSummary(
    replyCount: (payload['reply_count'] as num).toInt(),
    descendantCount: (payload['descendant_count'] as num).toInt(),
    lastReplyAt: (payload['last_reply_at'] as num?)?.toInt(),
    participantPubkeys: participants is List
        ? participants.whereType<String>().toList()
        : const [],
  );
}

ChannelWindowStore mergeLiveChannelWindowEvent(
  ChannelWindowStore current,
  NostrEvent event, {
  required bool isTimelineRow,
  bool retainOutsideWindow = false,
}) {
  // A reply doesn't reach the main timeline itself — the root's "N replies" row
  // comes from this summary event, which the relay re-emits on every reply. It
  // has to be merged, not stored as an aux event: it is a replaceable snapshot
  // keyed by root, not another row in the timeline.
  if (event.kind == EventKind.channelThreadSummary) {
    final rootId = event.getTagValue('e');
    if (rootId == null) return current;
    final ChannelWindowThreadSummary summary;
    try {
      summary = parseChannelWindowThreadSummary(event);
    } catch (_) {
      return current;
    }
    final existing = current.liveThreadSummaries[rootId];
    // A newer timestamp wins. Equal timestamps are ordered by live delivery:
    // two mutations can produce relay-signed recounts in the same second, and
    // the later delivery is the authoritative snapshot for that root.
    if (existing != null && existing.createdAt > event.createdAt) {
      return current;
    }
    return current.copyWith(
      liveThreadSummaries: {
        ...current.liveThreadSummaries,
        rootId: ChannelWindowLiveThreadSummary(
          summary: summary,
          createdAt: event.createdAt,
        ),
      },
    );
  }

  if (!isTimelineRow) {
    final alreadyKnown =
        current.liveAux.any((candidate) => candidate.id == event.id) ||
        current.allPages.any(
          (page) => page.aux.any((candidate) => candidate.id == event.id),
        );
    if (alreadyKnown) return current;
    return current.copyWith(liveAux: [...current.liveAux, event]);
  }

  final inPages = current.allPages.any(
    (page) => page.rows.any((row) => row.event.id == event.id),
  );
  if (inPages) return current;
  // Past an unloaded gap a row cannot show yet; the newer pages decide.
  if (_holdsLive(current, event)) {
    if (current.heldLive.any((candidate) => candidate.id == event.id)) {
      return current;
    }
    return ChannelWindowStore(
      pages: current.pages,
      newerPages: current.newerPages,
      anchorCursor: current.anchorCursor,
      anchorId: current.anchorId,
      hasNewer: current.hasNewer,
      newerCursor: current.newerCursor,
      heldLive: _sortedLive([...current.heldLive, event]),
      liveOverlay: current.liveOverlay,
      liveAux: current.liveAux,
      liveThreadSummaries: current.liveThreadSummaries,
    );
  }
  final oldestPage = current.pages.isEmpty ? null : current.pages.last;
  final oldest = oldestPage?.rows.isEmpty ?? true
      ? null
      : oldestPage!.rows.last.event;
  if (!retainOutsideWindow &&
      oldest != null &&
      (event.createdAt < oldest.createdAt ||
          (oldestPage!.hasMore && _compareRelayOrder(event, oldest) >= 0))) {
    return current;
  }
  final overlay =
      current.liveOverlay
          .where((candidate) => candidate.id != event.id)
          .toList()
        ..add(event)
        ..sort(_compareRelayOrder);
  return current.copyWith(liveOverlay: overlay);
}

List<NostrEvent> flattenChannelWindowEvents(ChannelWindowStore store) {
  final byId = <String, NostrEvent>{};
  for (final page in store.allPages) {
    for (final row in page.rows) {
      byId[row.event.id] = row.event;
    }
    for (final event in page.aux) {
      byId[event.id] = event;
    }
  }
  for (final event in store.liveOverlay) {
    byId[event.id] = event;
  }
  for (final event in store.liveAux) {
    byId[event.id] = event;
  }
  return byId.values.toList()
    ..sort(compareChannelTimelineEventsChronologically);
}

bool channelWindowHasMore(ChannelWindowStore store) =>
    store.pages.isNotEmpty && store.pages.last.hasMore;

ChannelPageCursor? channelWindowNextCursor(ChannelWindowStore store) =>
    store.pages.isEmpty ? null : store.pages.last.nextCursor;

Map<String, ChannelWindowThreadSummary> channelWindowThreadSummaries(
  ChannelWindowStore store,
) {
  return {
    for (final page in store.allPages)
      for (final row in page.rows)
        if (row.thread != null) row.event.id: row.thread!,
    // Live entries last: they are the newer snapshot for any root they cover.
    for (final entry in store.liveThreadSummaries.entries)
      entry.key: entry.value.summary,
  };
}

Map<String, dynamic> _parseJsonMap(NostrEvent event, String label) {
  try {
    final decoded = jsonDecode(event.content);
    if (decoded is Map<String, dynamic>) return decoded;
  } catch (_) {}
  throw Exception('Invalid $label event ${event.id}.');
}

ChannelPageCursor? _parseCursor(Object? value) {
  if (value == null) return null;
  if (value is! Map<String, dynamic>) {
    throw Exception('Invalid channel window cursor.');
  }
  return ChannelPageCursor(
    createdAt: (value['created_at'] as num).toInt(),
    eventId: value['id'] as String,
  );
}

String _expectedBoundsKey(
  String channelId,
  ChannelPageCursor? cursor, {
  required bool newer,
}) {
  final suffix = cursor == null
      ? 'head'
      : '${newer ? 'after:' : ''}'
            '${cursor.createdAt}:${cursor.eventId.toLowerCase()}';
  return '${channelId.toLowerCase()}:$suffix';
}

bool _cursorsEqual(ChannelPageCursor? left, ChannelPageCursor? right) =>
    identical(left, right) ||
    (left != null &&
        right != null &&
        left.createdAt == right.createdAt &&
        left.eventId == right.eventId);

void _assertValidPage(ChannelWindowPage page) {
  final newer = page.direction == WindowPageDirection.newer;
  if (newer && page.startCursor == null) {
    throw Exception('A newer channel page needs a cursor.');
  }
  if (page.hasMore != (page.nextCursor != null)) {
    throw Exception('Channel window hasMore and nextCursor disagree.');
  }
  final seen = <String>{};
  for (var index = 0; index < page.rows.length; index++) {
    final event = page.rows[index].event;
    if (!seen.add(event.id)) {
      throw Exception('Duplicate channel row ${event.id}.');
    }
    final startCursor = page.startCursor;
    if (startCursor != null &&
        !(newer
            ? _isStrictlyNewer(event, startCursor)
            : _isStrictlyOlder(event, startCursor))) {
      throw Exception(
        'Channel row ${event.id} is outside its cursor interval.',
      );
    }
    // A newer page runs the mirrored order, oldest first.
    if (index > 0) {
      final order = _compareRelayOrder(page.rows[index - 1].event, event);
      if (newer ? order < 0 : order > 0) {
        throw Exception('Channel window rows are not in relay order.');
      }
    }
  }
}

bool _isStrictlyOlder(NostrEvent event, ChannelPageCursor cursor) =>
    event.createdAt < cursor.createdAt ||
    (event.createdAt == cursor.createdAt &&
        event.id.compareTo(cursor.eventId) > 0);

bool _isStrictlyNewer(NostrEvent event, ChannelPageCursor cursor) =>
    event.createdAt > cursor.createdAt ||
    (event.createdAt == cursor.createdAt &&
        event.id.compareTo(cursor.eventId) < 0);

int _compareRelayOrder(NostrEvent left, NostrEvent right) {
  if (left.createdAt != right.createdAt) {
    return right.createdAt - left.createdAt;
  }
  return left.id.compareTo(right.id);
}
