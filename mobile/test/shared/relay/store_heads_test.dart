import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/relay/nostr_models.dart';
import 'package:kuzz/shared/relay/room_bundle_coverage.dart';

NostrEvent _event(String id, int kind, List<List<String>> tags, [Object? c]) =>
    NostrEvent(
      id: id,
      pubkey: 'p',
      createdAt: 1000,
      kind: kind,
      tags: tags,
      content: c == null ? '' : jsonEncode(c),
      sig: null,
    );

NostrFilter _roomWindow(String room, {int? until}) => NostrFilter(
  kinds: EventKind.channelTimelineContentKinds,
  tags: {
    '#h': [room],
  },
  limit: 50,
  until: until,
  extensions: {
    'top_level': true,
    'include_summaries': true,
    'include_aux': true,
    'before_id': ?(until == null ? null : 'z'),
  },
);

NostrFilter _threadWindow(String room, String root, {int limit = 50}) =>
    NostrFilter(
      kinds: const [9, 40002],
      tags: {
        '#h': [room],
        '#e': [root],
      },
      limit: limit,
      extensions: const {'thread_window': true, 'include_aux': false},
    );

void main() {
  test('only newest room and thread windows are heads', () {
    expect(storeHeadWindowOf(_roomWindow('r1'))?.roomId, 'r1');
    expect(storeHeadWindowOf(_roomWindow('r1', until: 900)), isNull);
    final thread = storeHeadWindowOf(_threadWindow('r1', 't1'));
    expect((thread?.roomId, thread?.rootId), ('r1', 't1'));
    expect(
      storeHeadWindowOf(
        const NostrFilter(
          kinds: [9],
          tags: {
            '#h': ['r1'],
          },
        ),
      ),
      isNull,
    );
  });

  test('newest room windows ask for the writer; thread windows do not', () {
    expect(
      storeHeadFilter(_roomWindow('r1')).toJson()['consistency'],
      'strong',
    );
    expect(
      storeHeadFilter(_roomWindow('r1', until: 900)).toJson(),
      isNot(contains('consistency')),
    );
    // The thread-window parser rejects unknown fields.
    expect(
      storeHeadFilter(_threadWindow('r1', 't1')).toJson(),
      isNot(contains('consistency')),
    );
  });

  test('answers split at their bounds', () {
    final windows = [
      storeHeadWindowOf(_roomWindow('r1'))!,
      storeHeadWindowOf(_threadWindow('r1', 't1'))!,
    ];
    final answers = storeHeadAnswers(windows, [
      _event('a', 9, []),
      _event('b', 9, []),
      _event(
        'bounds-r1',
        39006,
        [
          ['d', 'r1:head'],
          ['h', 'r1'],
        ],
        {
          'has_more': true,
          'next_cursor': {'created_at': 900, 'id': 'b'},
        },
      ),
      _event('q', 9, []),
      _event(
        'bounds-t1',
        39007,
        [
          ['h', 'r1'],
          ['e', 't1'],
        ],
        {'has_more': false, 'next_cursor': null, 'direction': 'older'},
      ),
      // Bounds of a window not asked for are skipped.
      _event('x', 9, []),
      _event(
        'bounds-r2',
        39006,
        [
          ['d', 'r2:head'],
          ['h', 'r2'],
        ],
        {'has_more': false},
      ),
    ]);
    expect(answers, hasLength(2));
    expect(answers[0].window.rootId, isNull);
    expect(answers[0].nextCursor, (900, 'b'));
    expect(answers[0].ids, {'a', 'b'});
    expect(answers[1].window.rootId, 't1');
    expect(answers[1].hasMore, isFalse);
    expect(answers[1].ids, {'q'});
  });

  test('narrow, repeated or non-newest windows record nothing', () {
    // Narrower than the open window (the thread preload's 20 rows).
    expect(storeHeadWindowOf(_threadWindow('r1', 't1', limit: 20)), isNull);
    final t1 = storeHeadWindowOf(_threadWindow('r1', 't1'))!;
    final t2 = storeHeadWindowOf(_threadWindow('r1', 't2'))!;
    NostrEvent bounds(String root, [String direction = 'older']) => _event(
      'bounds-$root',
      39007,
      [
        ['h', 'r1'],
        ['e', root],
      ],
      {'has_more': false, 'direction': direction},
    );
    // A root asked twice.
    expect(storeHeadAnswers([t1, t1], [bounds('t1')]), isEmpty);
    // A root answered twice.
    expect(storeHeadAnswers([t1], [bounds('t1'), bounds('t1')]), isEmpty);
    // Bounds that do not page older.
    expect(storeHeadAnswers([t1, t2], [bounds('t1', 'newer'), bounds('t2')]), [
      isA<StoreHeadAnswer>().having((a) => a.window.rootId, 'root', 't2'),
    ]);
  });

  test('a head token holds only while its EOSE coverage does', () async {
    sqfliteFfiInit();
    final dir = await Directory.systemTemp.createTemp('store_heads');
    addTearDown(() => dir.delete(recursive: true));
    final store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => dir.path,
    );
    final lease = store.lease(
      'wss://relay.example',
      'aa00000000000000000000000000000000000000000000000000000000000001',
    )!;
    final coverage = RoomBundleCoverage(
      scope: () => (store: store, lease: lease),
      nowSecs: () => 10000,
      query: (_) async => <Object?>[],
    );
    final sub = Object();
    final filter = NostrFilter(
      kinds: EventKind.channelEventKinds,
      tags: const {
        '#h': ['r1'],
      },
      limit: 0,
    );
    expect(coverage.liveTokenOf(sub, 1), isNull);
    coverage.noteEose(sub, filter, 1);
    final token = coverage.liveTokenOf(sub, 1)!;
    expect(coverage.liveAtOfToken(token), 10000);
    expect(coverage.holds(sub, token, 1), isTrue);
    // A new connection, or a new EOSE, ends it.
    expect(coverage.holds(sub, token, 2), isFalse);
    coverage.noteEose(sub, filter, 1);
    expect(coverage.holds(sub, token, 1), isFalse);
    coverage.noteClosed(sub);
    expect(coverage.liveTokenOf(sub, 1), isNull);
    await store.flushed();
    await store.forget('wss://relay.example', lease.account);
  });
}
