import 'dart:convert';

import 'package:kuzz/features/channels/channel_window.dart';
import 'package:kuzz/features/channels/window_anchor.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';

import 'window_relay_fixture.dart';

const _ch = 'ABCDEF';

/// Rows 1..[count] in timeline order, three to a second so page edges split
/// seconds. Within a second a newer row has the lower id, as the relay orders
/// them; ids are spaced so no anchor cursor lands on a real row.
List<NostrEvent> _rows(int count) => [
  for (var n = 1; n <= count; n++) _rowAt(n, 1000 + n ~/ 3),
];

NostrEvent _rowAt(int n, int createdAt, {List<List<String>> tags = const []}) =>
    fixtureRow(_ch, (100000 - n) * 16, createdAt, extraTags: tags);

ChannelWindowPage _fetch(
  List<NostrEvent> rows,
  ChannelPageCursor? cursor, {
  WindowPageDirection direction = WindowPageDirection.older,
  int limit = 7,
}) {
  final newer = direction == WindowPageDirection.newer;
  final filter = NostrFilter(
    kinds: const [],
    limit: limit,
    until: newer ? null : cursor?.createdAt,
    since: newer ? cursor?.createdAt : null,
    extensions: {
      if (cursor != null) newer ? 'after_id' : 'before_id': cursor.eventId,
    },
  );
  return parseChannelWindowResponse(
    serveChannelWindow(_ch, rows, filter),
    _ch,
    cursor,
    direction: direction,
  );
}

ChannelPageCursor _anchor(int createdAt, [String? eventId]) {
  final anchor = windowAnchorCursor(createdAt: createdAt, eventId: eventId);
  return ChannelPageCursor(
    createdAt: anchor.createdAt,
    eventId: anchor.eventId,
  );
}

ChannelWindowStore _openAt(
  List<NostrEvent> rows,
  ChannelPageCursor cursor, {
  ChannelWindowStore current = const ChannelWindowStore.empty(),
  String? anchorId,
  int limit = 7,
}) => openMiddleChannelWindow(
  current,
  _fetch(rows, cursor, limit: limit),
  _fetch(rows, cursor, direction: WindowPageDirection.newer, limit: limit),
  anchorId: anchorId,
);

ChannelWindowStore _pageToPresent(
  List<NostrEvent> rows,
  ChannelWindowStore store, {
  int limit = 7,
}) {
  var current = store;
  for (var i = 0; current.hasNewer; i++) {
    expect(i, lessThan(1000), reason: 'newer paging must terminate');
    current = appendNewerChannelWindow(
      current,
      _fetch(
        rows,
        current.newerCursor,
        direction: WindowPageDirection.newer,
        limit: limit,
      ),
    );
  }
  return current;
}

List<String> _flatIds(ChannelWindowStore store) =>
    flattenChannelWindowEvents(store).map((event) => event.id).toList();

/// Chronological ids of [rows] from [from] onward, as the timeline shows them.
List<String> _chronological(List<NostrEvent> rows, {NostrEvent? from}) {
  final sorted = rows.toList()..sort((a, b) => compareRelayOrder(b, a));
  final start = from == null ? 0 : sorted.indexOf(from);
  return sorted.skip(start).map((event) => event.id).toList();
}

NostrEvent _bounds(String suffix, Map<String, Object?> content) => NostrEvent(
  id: 'bounds',
  pubkey: 'relay',
  createdAt: 0,
  kind: EventKind.channelWindowBounds,
  tags: [
    ['d', '${_ch.toLowerCase()}:$suffix'],
  ],
  content: jsonEncode(content),
  sig: '',
);

void main() {
  group('windowAnchorCursor', () {
    test('a message anchor sits just before the message in relay order', () {
      final cursor = windowAnchorCursor(createdAt: 5, eventId: fixtureId(16));
      expect(cursor.createdAt, 5);
      expect(cursor.eventId, fixtureId(15));
    });

    test('a read marker, or an unusable id, uses the zero id', () {
      for (final id in [null, 'not-hex', fixtureId(0)]) {
        expect(
          windowAnchorCursor(createdAt: 5, eventId: id).eventId,
          zeroEventId,
        );
      }
    });
  });

  group('parseChannelWindowResponse newer pages', () {
    final rows = _rows(9);
    final cursor = ChannelPageCursor.of(rows[3]);

    test('reads an after: bounds key that names the newer direction', () {
      final page = _fetch(
        rows,
        cursor,
        direction: WindowPageDirection.newer,
        limit: 3,
      );
      expect(page.direction, WindowPageDirection.newer);
      expect(page.rows.map((row) => row.event.id), [
        rows[4].id,
        rows[5].id,
        rows[6].id,
      ]);
      expect(page.hasMore, isTrue);
      expect(page.nextCursor?.eventId, rows[6].id);
    });

    test('rejects bounds for another cursor form or direction', () {
      final key = '${cursor.createdAt}:${cursor.eventId}';
      for (final (suffix, content) in [
        // An older bounds key answering a newer request.
        (key, {'direction': 'newer', 'has_more': false, 'next_cursor': null}),
        // A relay that ignored the direction.
        ('after:$key', {'has_more': false, 'next_cursor': null}),
        ('after:$key', {'direction': 'older', 'has_more': false}),
      ]) {
        expect(
          () => parseChannelWindowResponse(
            [_bounds(suffix, content)],
            _ch,
            cursor,
            direction: WindowPageDirection.newer,
          ),
          throwsException,
          reason: '$suffix $content',
        );
      }
      expect(
        () => parseChannelWindowResponse(
          [
            _bounds(key, {
              'direction': 'newer',
              'has_more': false,
              'next_cursor': null,
            }),
          ],
          _ch,
          cursor,
        ),
        throwsException,
        reason: 'an older request must not accept newer bounds',
      );
    });

    test(
      'a window rejects rows out of mirrored order or not after the cursor',
      () {
        final store = _openAt(rows, cursor, limit: 1);
        expect(store.newerCursor?.eventId, rows[4].id);
        for (final pageRows in [
          [rows[6], rows[5]],
          [rows[4]],
          [rows[2]],
        ]) {
          expect(
            () => appendNewerChannelWindow(
              store,
              ChannelWindowPage(
                startCursor: store.newerCursor,
                rows: [
                  for (final event in pageRows) ChannelWindowRow(event: event),
                ],
                aux: const [],
                nextCursor: null,
                hasMore: false,
                direction: WindowPageDirection.newer,
              ),
            ),
            throwsException,
          );
        }
      },
    );
  });

  group('middle window', () {
    test('opens with the anchor leading the older page', () {
      final rows = _rows(40);
      final anchor = rows[19];
      final store = _openAt(
        rows,
        _anchor(anchor.createdAt, anchor.id),
        anchorId: anchor.id,
      );
      expect(store.isMiddle, isTrue);
      expect(store.anchorId, anchor.id);
      expect(store.pages.single.rows.first.event.id, anchor.id);
      expect(store.hasNewer, isTrue);
      // Anchor plus a page each way, contiguous, each row once.
      final sorted = _chronological(rows);
      final at = sorted.indexOf(anchor.id);
      expect(_flatIds(store), sorted.sublist(at - 6, at + 8));
    });

    test('pages from an old message down to the newest without gaps', () {
      final rows = _rows(100);
      final anchor = rows[10];
      final opened = _openAt(rows, _anchor(anchor.createdAt, anchor.id));
      final complete = _pageToPresent(rows, opened);
      expect(complete.hasNewer, isFalse);
      expect(complete.newerCursor, isNull);
      final firstOlder = opened.pages.single.rows.last.event;
      expect(_flatIds(complete), _chronological(rows, from: firstOlder));
      expect(complete.newerPages.length, greaterThan(10));
    });

    test('a read marker splits its second: older <= ts, newer > ts', () {
      final rows = _rows(30);
      const ts = 1005; // rows 15, 16, 17
      final store = _openAt(rows, _anchor(ts), limit: 50);
      final older = store.pages.single.rows.map((row) => row.event);
      final newer = store.newerPages.single.rows.map((row) => row.event);
      expect(older.every((event) => event.createdAt <= ts), isTrue);
      expect(older.where((event) => event.createdAt == ts).length, 3);
      expect(newer.every((event) => event.createdAt > ts), isTrue);
      expect(older.length + newer.length, rows.length);
    });

    test('holds live rows until the newer pages reach the present', () {
      final rows = _rows(60);
      var store = _openAt(rows, _anchor(rows[5].createdAt, rows[5].id));
      final live = _rowAt(500, 2000);
      final reply = _rowAt(
        501,
        2001,
        tags: [
          ['e', rows[5].id, '', 'reply'],
        ],
      );
      store = mergeLiveChannelWindowEvent(store, live, isTimelineRow: true);
      store = mergeLiveChannelWindowEvent(store, reply, isTimelineRow: true);
      expect(store.heldLive.map((event) => event.id), [live.id]);
      // Reply evidence is not a timeline row past the gap.
      expect(store.liveOverlay.map((event) => event.id), [reply.id]);
      expect(_flatIds(store), isNot(contains(live.id)));
      // A repeat delivery is not held twice.
      expect(
        mergeLiveChannelWindowEvent(store, live, isTimelineRow: true),
        same(store),
      );

      store = _pageToPresent(rows, store);
      expect(store.heldLive, isEmpty);
      final ids = _flatIds(store);
      expect(ids.where((id) => id == live.id).length, 1);
      expect(ids.last, reply.id);
    });

    test('a held row the newer page also returns shows once', () {
      final rows = _rows(30);
      var store = _openAt(rows, _anchor(rows[2].createdAt, rows[2].id));
      // Delivered live while its page was still to come.
      final late = rows.last;
      store = mergeLiveChannelWindowEvent(store, late, isTimelineRow: true);
      expect(store.heldLive.single.id, late.id);
      store = _pageToPresent(rows, store);
      expect(store.liveOverlay, isEmpty);
      expect(_flatIds(store).where((id) => id == late.id).length, 1);
    });

    test('same-second rows on both sides of a page edge stay distinct', () {
      // Seven rows in one second: every page edge falls inside it.
      final rows = [for (var n = 1; n <= 7; n++) _rowAt(n, 1000)];
      final opened = _openAt(rows, _anchor(1000, rows[3].id), limit: 2);
      expect(opened.pages.single.rows.first.event.id, rows[3].id);
      final store = _pageToPresent(rows, opened, limit: 2);
      expect(
        _flatIds(store),
        _chronological(rows, from: opened.pages.single.rows.last.event),
      );
    });

    test('rejects a newer page that does not continue the chain', () {
      final rows = _rows(40);
      final store = _openAt(rows, _anchor(rows[5].createdAt, rows[5].id));
      expect(
        () => appendNewerChannelWindow(
          store,
          _fetch(
            rows,
            ChannelPageCursor.of(rows[30]),
            direction: WindowPageDirection.newer,
          ),
        ),
        throwsException,
      );
      expect(
        () => appendOlderChannelWindow(
          store,
          _fetch(rows, store.newerCursor, direction: WindowPageDirection.newer),
        ),
        throwsException,
      );
    });

    test('older pages still extend a middle window backward', () {
      final rows = _rows(40);
      var store = _openAt(rows, _anchor(rows[20].createdAt, rows[20].id));
      while (channelWindowHasMore(store)) {
        store = appendOlderChannelWindow(
          store,
          _fetch(rows, channelWindowNextCursor(store)),
        );
      }
      store = _pageToPresent(rows, store);
      expect(_flatIds(store), _chronological(rows));
    });

    test('the newest page ends the middle window and keeps held rows', () {
      final rows = _rows(40);
      var store = _openAt(rows, _anchor(rows[5].createdAt, rows[5].id));
      final live = _rowAt(900, 3000);
      store = mergeLiveChannelWindowEvent(store, live, isTimelineRow: true);
      store = replaceNewestChannelWindow(store, _fetch(rows, null));
      expect(store.isMiddle, isFalse);
      expect(store.newerPages, isEmpty);
      expect(store.heldLive, isEmpty);
      expect(store.liveOverlay.single.id, live.id);
      expect(_flatIds(store).last, live.id);
    });

    test('a reconnect pages on from the newest loaded row', () {
      final rows = _rows(20);
      final anchor = _anchor(rows[5].createdAt, rows[5].id);
      final complete = _pageToPresent(rows, _openAt(rows, anchor));
      final reopened = reopenNewerChannelWindowEdge(complete);
      expect(reopened.hasNewer, isTrue);
      expect(reopened.newerCursor?.eventId, rows.last.id);
      // Nothing was missed: the next page is empty and closes the gap again.
      final next = _pageToPresent(rows, reopened);
      expect(next.hasNewer, isFalse);
      expect(_flatIds(next), _flatIds(complete));

      // With no newer row loaded the anchor is the edge.
      final atNewest = _openAt(rows, _anchor(rows.last.createdAt + 1));
      expect(
        reopenNewerChannelWindowEdge(atNewest).newerCursor?.eventId,
        zeroEventId,
      );
      // A window opened at the newest row has no edge to reopen.
      final head = replaceNewestChannelWindow(atNewest, _fetch(rows, null));
      expect(reopenNewerChannelWindowEdge(head), same(head));
    });
  });

  group('store rows', () {
    final rows = _rows(40);
    final store = ChannelWindowStore(
      pages: [_fetch(rows.sublist(0, 30), null)],
      liveThreadSummaries: const {},
      liveOverlay: const [],
      liveAux: const [],
    );
    // A row of the store, past what the relay has answered with.
    final stored = foldStoredChannelRows(store, [rows[35]]);

    List<String> shown(ChannelWindowStore window) => [
      for (final event in flattenChannelWindowEvents(window)) event.id,
    ];

    test(
      'a fold shows rows, once, and leaves a window with nothing to add',
      () {
        expect(shown(stored), contains(rows[35].id));
        expect(stored.storeRows, hasLength(1));
        expect(
          identical(foldStoredChannelRows(stored, [rows[35]]), stored),
          true,
        );
        expect(
          identical(foldStoredChannelRows(stored, [rows[29]]), stored),
          true,
        );
      },
    );

    test(
      'a row older than the window while older pages remain is left out',
      () {
        // The window holds the newest 7 of 30: rows 24 to 30.
        expect(
          identical(foldStoredChannelRows(stored, [rows[2]]), stored),
          true,
        );
      },
    );

    test('every relay fold toward the present drops them', () {
      expect(
        replaceNewestChannelWindow(stored, _fetch(rows, null)).storeRows,
        isEmpty,
      );
      final anchor = _anchor(rows[10].createdAt, rows[10].id);
      final older = _fetch(rows, anchor);
      final newer = _fetch(rows, anchor, direction: WindowPageDirection.newer);
      final middle = openMiddleChannelWindow(stored, older, newer);
      expect(middle.storeRows, isEmpty);

      final again = foldStoredChannelRows(middle, [rows[35]]);
      expect(again.storeRows, hasLength(1));
      final next = _fetch(
        rows,
        again.newerCursor,
        direction: WindowPageDirection.newer,
      );
      expect(appendNewerChannelWindow(again, next).storeRows, isEmpty);
    });

    test('an older page keeps them, less any it holds', () {
      final tail = store.pages.last.nextCursor;
      final page = _fetch(rows.sublist(0, 30), tail);
      expect(appendOlderChannelWindow(stored, page).storeRows, hasLength(1));
      final holding = foldStoredChannelRows(
        ChannelWindowStore(
          pages: [
            ChannelWindowPage(
              startCursor: null,
              rows: store.pages.first.rows,
              aux: const [],
              nextCursor: tail,
              hasMore: true,
            ),
          ],
          liveThreadSummaries: const {},
          liveOverlay: const [],
          liveAux: const [],
        ),
        [page.rows.first.event],
      );
      // Past the oldest loaded row, so the fold above placed nothing.
      expect(holding.storeRows, isEmpty);
      final withRow = holding.copyWith(storeRows: [page.rows.first.event]);
      expect(appendOlderChannelWindow(withRow, page).storeRows, isEmpty);
    });

    test('live rows carry them, and a live copy of one takes its place', () {
      final anchor = _anchor(rows[10].createdAt, rows[10].id);
      final held = foldStoredChannelRows(
        openMiddleChannelWindow(
          store,
          _fetch(rows, anchor),
          _fetch(rows, anchor, direction: WindowPageDirection.newer),
        ),
        [rows[35]],
      );
      expect(held.hasNewer, isTrue);
      expect(
        mergeLiveChannelWindowEvent(
          held,
          rows[38],
          isTimelineRow: true,
        ).storeRows,
        hasLength(1),
      );

      final live = mergeLiveChannelWindowEvent(
        stored,
        rows[35],
        isTimelineRow: true,
      );
      expect(live.storeRows, isEmpty);
      expect(live.liveOverlay.single.id, rows[35].id);
    });

    test('reopening a middle window edge keeps them until its page', () {
      final anchor = _anchor(rows[10].createdAt, rows[10].id);
      final atPresent = openMiddleChannelWindow(
        store,
        _fetch(rows, anchor, limit: 40),
        _fetch(rows, anchor, direction: WindowPageDirection.newer, limit: 40),
      );
      expect(atPresent.hasNewer, isFalse);
      final folded = foldStoredChannelRows(atPresent, [
        fixtureRow(_ch, 16, 2000),
      ]);
      final reopened = reopenNewerChannelWindowEdge(folded);
      expect(reopened.hasNewer, isTrue);
      expect(reopened.storeRows, hasLength(1));
      expect(shown(reopened), contains(folded.storeRows.single.id));
    });
  });
}
