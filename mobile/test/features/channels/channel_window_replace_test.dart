import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/channel_window.dart';
import 'package:kuzz/features/channels/window_anchor.dart';
import 'package:kuzz/shared/relay/relay.dart';

// Shared rules with desktop (test-fixtures/channel-window-replace.json).

NostrEvent _message(List<dynamic> row) => NostrEvent(
  id: row[0] as String,
  pubkey: 'alice',
  createdAt: row[1] as int,
  kind: EventKind.streamMessage,
  tags: const [
    ['h', 'room'],
  ],
  content: '',
  sig: '',
);

NostrEvent _reaction(List<dynamic> entry) => NostrEvent(
  id: entry[0] as String,
  pubkey: 'alice',
  createdAt: 1,
  kind: EventKind.reaction,
  tags: [
    const ['h', 'room'],
    ['e', entry[1] as String],
  ],
  content: '+',
  sig: '',
);

const _summary = ChannelWindowThreadSummary(
  replyCount: 1,
  descendantCount: 1,
  lastReplyAt: null,
  participantPubkeys: [],
);

ChannelPageCursor? _cursorOf(List<dynamic>? row) => row == null
    ? null
    : ChannelPageCursor(createdAt: row[1] as int, eventId: row[0] as String);

ChannelWindowPage _page(Map<String, dynamic> spec, ChannelPageCursor? start) {
  final rows = (spec['rows'] as List).cast<List<dynamic>>();
  final threads = (spec['threads'] as Map<String, dynamic>?) ?? const {};
  final more = spec['more'] as bool;
  return ChannelWindowPage(
    startCursor: start,
    rows: [
      for (final row in rows)
        threads.containsKey(row[0])
            ? ChannelWindowRow(
                event: _message(row),
                thread: _summary,
                threadSummaryCreatedAt: threads[row[0]] as int,
              )
            : ChannelWindowRow(event: _message(row)),
    ],
    aux: [
      for (final entry in (spec['aux'] as List?) ?? const [])
        _reaction(entry as List<dynamic>),
    ],
    nextCursor: more ? _cursorOf(rows.last) : null,
    hasMore: more,
  );
}

ChannelWindowStore _storeOf(Map<String, dynamic> testCase) {
  var store = const ChannelWindowStore.empty();
  final current = (testCase['current'] as List).cast<Map<String, dynamic>>();
  for (var index = 0; index < current.length; index++) {
    store = index == 0
        ? replaceNewestChannelWindow(store, _page(current[index], null))
        : appendOlderChannelWindow(
            store,
            _page(current[index], store.pages.last.nextCursor),
          );
  }
  final live = (testCase['live_summaries'] as List?)?.cast<String>() ?? [];
  return store.copyWith(
    liveThreadSummaries: {
      for (final rootId in live)
        rootId: const ChannelWindowLiveThreadSummary(
          summary: _summary,
          createdAt: 1,
        ),
    },
  );
}

List<Object?>? _cursorJson(ChannelPageCursor? cursor) =>
    cursor == null ? null : [cursor.createdAt, cursor.eventId];

void main() {
  final fixture =
      jsonDecode(
            File(
              '../test-fixtures/channel-window-replace.json',
            ).readAsStringSync(),
          )
          as Map<String, dynamic>;
  for (final raw in fixture['cases'] as List) {
    final testCase = raw as Map<String, dynamic>;
    test('shared fixture: ${testCase['name']}', () {
      var store = replaceNewestChannelWindow(
        _storeOf(testCase),
        _page(testCase['page'] as Map<String, dynamic>, null),
      );
      final older = testCase['then_older'] as Map<String, dynamic>?;
      if (older != null) {
        store = appendOlderChannelWindow(
          store,
          _page(older, store.pages.last.nextCursor),
        );
      }
      final expected = testCase['expect'] as Map<String, dynamic>;
      expect(
        jsonEncode([
          for (final page in store.pages)
            {
              'start': _cursorJson(page.startCursor),
              'rows': [for (final row in page.rows) row.event.id],
              'aux': [for (final event in page.aux) event.id],
              'next': _cursorJson(page.nextCursor),
            },
        ]),
        jsonEncode(expected['pages']),
      );
      final live = expected['live_summaries'] as List?;
      if (live != null) {
        expect(store.liveThreadSummaries.keys.toList()..sort(), live);
      }
    });
  }

  test('a window opened at a message starts over at the newest page', () {
    final older = ChannelWindowPage(
      startCursor: const ChannelPageCursor(createdAt: 100, eventId: 'a'),
      rows: [
        ChannelWindowRow(event: _message(['a0', 90])),
      ],
      aux: const [],
      nextCursor: null,
      hasMore: false,
    );
    final newer = ChannelWindowPage(
      startCursor: const ChannelPageCursor(createdAt: 100, eventId: 'a'),
      rows: [
        ChannelWindowRow(event: _message(['b', 105])),
      ],
      aux: const [],
      nextCursor: null,
      hasMore: false,
      direction: WindowPageDirection.newer,
    );
    final middle = openMiddleChannelWindow(
      const ChannelWindowStore.empty(),
      older,
      newer,
      anchorId: 'a',
    );
    final store = replaceNewestChannelWindow(
      middle,
      _page({
        'rows': [
          ['n', 110],
          ['b', 105],
        ],
        'more': true,
      }, null),
    );
    expect(store.isMiddle, isFalse);
    expect(store.pages, hasLength(1));
  });
}
