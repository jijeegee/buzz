import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/shared/auth/auth_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  group('channel order numbers', () {
    test('only the connect URL asks for order numbers', () {
      expect(
        RelaySocket.connectUri(
          'wss://relay.example',
          sequenceFrames: false,
        ).toString(),
        'wss://relay.example',
      );
      expect(
        RelaySocket.connectUri(
          'wss://relay.example/ws?x=1',
          sequenceFrames: true,
        ).queryParameters,
        {'x': '1', 'kuzz_seq': '1'},
      );
    });

    test('tells page bounds apart from window bounds', () {
      final page = _pageBounds(after: 4, last: 9, hasMore: true, ids: ['a']);
      final parsed = parseAfterSeqBounds(page)!;
      expect(parsed.lastSeq, 9);
      expect(parsed.hasMore, isTrue);
      expect(parsed.sequences, {'a': 9});
      expect(isAfterSeqBounds(page), isTrue);

      final window = _event(
        'window',
        kind: EventKind.channelWindowBounds,
        tags: [
          ['d', '$_channelId:head'],
        ],
        content: jsonEncode({'has_more': false, 'next_cursor': null}),
      );
      expect(parseAfterSeqBounds(window), isNull);
      expect(isAfterSeqBounds(window), isFalse);
      expect(
        parseAfterSeqBounds(
          _event('message', content: jsonEncode({'order': 'seq'})),
        ),
        isNull,
      );
    });

    test('repair starts from a number that has been newest long enough', () {
      final t0 = DateTime(2026, 10, 21);
      final cursor = ChannelSeqCursor(10, t0);
      expect(cursor.repairPoint(t0), 9, reason: 'just before the first seen');
      cursor.observe(12, t0.add(const Duration(seconds: 1)));
      expect(cursor.repairPoint(t0.add(const Duration(seconds: 5))), 9);
      // A gap right before a resync may already be behind the newest (12).
      expect(cursor.repairPoint(t0.add(const Duration(seconds: 31))), 10);
      cursor.observe(20, t0.add(const Duration(seconds: 40)));
      expect(cursor.repairPoint(t0.add(const Duration(seconds: 71))), 12);
      expect(cursor.newest, 20);
    });

    test('repair skips kinds without order numbers', () {
      expect(afterSeqKinds([9, 9, 20001, 20002, 40902, 40002]), [9, 40002]);
    });

    test('connecting never waits on NIP-11', () async {
      final info = Completer<RelayInfo>();
      var reads = 0;
      final first = await _Harness.start(
        sequenced: false,
        probe: true,
        fetcher: (_) {
          reads++;
          return info.future;
        },
      );
      // Nothing known yet: connect without order numbers, read in background.
      expect(await first.session.debugRelaySpeaksSequence(_baseUrl), isFalse);
      expect(await first.session.debugRelaySpeaksSequence(_baseUrl), isFalse);
      expect(reads, 1);
      info.complete(const RelayInfo(supportedExtensions: {kuzzSeqExtension}));
      await Future<void>.delayed(Duration.zero);
      expect(await first.session.debugRelaySpeaksSequence(_baseUrl), isTrue);

      // The next app run starts from the stored answer, while the relay
      // (here a failing read) is asked again in the background.
      final second = await _Harness.start(
        sequenced: false,
        probe: true,
        fetcher: (_) async => throw StateError('offline'),
      );
      expect(await second.session.debugRelaySpeaksSequence(_baseUrl), isTrue);
    });

    test('live filters match like the relay', () {
      final event = _event(
        'a',
        tags: [
          ['h', _channelId],
          ['e', 'root'],
        ],
      );
      bool matches(NostrFilter filter) => liveFilterMatches(filter, event);
      const kinds = [EventKind.streamMessageV2];
      expect(
        matches(
          const NostrFilter(
            kinds: kinds,
            tags: {
              '#h': [_channelId],
            },
          ),
        ),
        isTrue,
      );
      expect(
        matches(
          const NostrFilter(
            kinds: kinds,
            tags: {
              '#h': [_channelId],
              '#e': ['other'],
            },
          ),
        ),
        isFalse,
      );
      expect(
        matches(const NostrFilter(kinds: kinds, authors: ['bob'])),
        isFalse,
      );
      expect(matches(const NostrFilter(kinds: kinds, ids: ['b'])), isFalse);
      expect(matches(const NostrFilter(kinds: kinds, since: 21)), isFalse);
      expect(matches(const NostrFilter(kinds: kinds, until: 19)), isFalse);
      expect(matches(const NostrFilter(kinds: [9])), isFalse);
    });

    test('one repair run covers at most 20 channels', () async {
      final channels = [
        for (var i = 0; i < 25; i++)
          '00000000-0000-4000-8000-${i.toString().padLeft(12, '0')}',
      ];
      final harness = await _Harness.start(channels: channels);
      for (var i = 0; i < channels.length; i++) {
        harness.live(
          _event(
            'm$i',
            tags: [
              ['h', channels[i]],
            ],
          ),
          sequence: 1,
        );
      }
      harness.session.debugHandleMessage([
        'KUZZ_RESYNC',
        {'reason': 'lag'},
      ]);
      final asked = <String>[];
      while (harness.historyReqs.length > asked.length) {
        final req = harness.historyReqs[asked.length];
        final channel = ((req[2] as Map<String, dynamic>)['#h'] as List).single;
        asked.add(channel as String);
        await harness.answer(req[1] as String, [
          _pageBounds(
            after: 0,
            last: 1,
            hasMore: false,
            ids: [],
            channel: channel,
          ),
        ]);
      }
      expect(asked, hasLength(20));
      // Most recently active first.
      expect(asked.first, channels.last);
    });

    test('a resync during repair runs once more after it', () async {
      final harness = await _Harness.start();
      harness.live(_message('a'), sequence: 5);
      harness.session.debugHandleMessage([
        'KUZZ_RESYNC',
        {'reason': 'lag'},
      ]);
      harness.session.debugHandleMessage([
        'KUZZ_RESYNC',
        {'reason': 'lag'},
      ]);
      expect(harness.historyReqs, hasLength(1));
      await harness.answer(harness.historyReqs.single[1] as String, [
        _pageBounds(after: 4, last: 5, hasMore: false, ids: ['a']),
      ]);
      expect(harness.historyReqs, hasLength(2));
      await harness.answer(harness.historyReqs.last[1] as String, [
        _pageBounds(after: 4, last: 5, hasMore: false, ids: []),
      ]);
      expect(harness.historyReqs, hasLength(2));
    });

    test('reads order numbers only on a sequenced connection', () async {
      final harness = await _Harness.start(sequenced: false);
      harness.live(_message('a'), sequence: 5);
      expect(harness.delivered.single.channelSequence, isNull);

      harness.session.debugHandleMessage([
        'KUZZ_RESYNC',
        {'reason': 'lag'},
      ]);
      expect(harness.historyReqs, isEmpty);
      expect(harness.session.debugSequenceRepairPoint(_channelId), isNull);
    });

    test('KUZZ_RESYNC pages the channel after its order number', () async {
      final harness = await _Harness.start();
      harness.live(_message('a'), sequence: 5);
      expect(harness.delivered.single.channelSequence, 5);

      harness.session.debugHandleMessage([
        'KUZZ_RESYNC',
        {'reason': 'lag'},
      ]);
      final first = harness.historyReqs.single;
      final filter = first[2] as Map<String, dynamic>;
      expect(filter['after_seq'], 4);
      expect(filter['limit'], afterSeqPageLimit);
      expect(filter['#h'], [_channelId]);
      expect(filter.containsKey('since'), isFalse);

      // Page 1: "a" again (deduplicated) and "b"; more to come.
      await harness.answer(first[1] as String, [
        _message('a'),
        _message('b'),
        _pageBounds(after: 4, last: 6, hasMore: true, ids: ['a', 'b']),
      ]);
      final second = harness.historyReqs.last;
      expect((second[2] as Map<String, dynamic>)['after_seq'], 6);
      await harness.answer(second[1] as String, [
        _message('c'),
        _pageBounds(after: 6, last: 7, hasMore: false, ids: ['c']),
      ]);

      expect(harness.historyReqs, hasLength(2));
      expect(harness.delivered.map((event) => event.id), ['a', 'b', 'c']);
      expect(harness.delivered.map((event) => event.channelSequence), [
        5,
        6,
        7,
      ]);
    });

    test('a reconnect fills what the gap missed', () async {
      final harness = await _Harness.start();
      harness.live(_message('a'), sequence: 5);
      harness.socket.messages.clear();

      final connected = harness.session.debugHandleConnected();
      await Future<void>.delayed(Duration.zero);
      final repair = harness.historyReqs.single;
      expect((repair[2] as Map<String, dynamic>)['after_seq'], 4);
      await harness.answer(repair[1] as String, [
        _message('b'),
        _pageBounds(after: 4, last: 6, hasMore: false, ids: ['a', 'b']),
      ]);
      await connected;
      expect(harness.delivered.map((event) => event.id), ['a', 'b']);
    });

    test('an accepted send records its order number', () async {
      final harness = await _Harness.start();
      final sent = harness.session.publish(_message('mine'));
      harness.session.debugHandleMessage([
        'OK',
        'mine',
        true,
        '',
        {'channel_sequence': 9},
      ]);
      await sent;
      expect(harness.session.debugSequenceRepairPoint(_channelId), 8);
    });

    test('page bounds never reach a live subscription', () async {
      final harness = await _Harness.start();
      harness.session.debugHandleMessage([
        'EVENT',
        'l-1',
        _pageBounds(after: 0, last: 1, hasMore: false, ids: []).toJson(),
      ]);
      harness.session.debugFlushEventBuffer();
      expect(harness.delivered, isEmpty);
    });
  });
}

const _channelId = '11111111-1111-4111-8111-111111111111';
const _baseUrl = 'https://relay.example';

class _Harness {
  _Harness(this.session, this.socket);

  final RelaySessionNotifier session;
  final _RecordingRelaySocket socket;
  final List<NostrEvent> delivered = [];

  static Future<_Harness> start({
    bool sequenced = true,
    bool probe = false,
    RelayInfoFetcher? fetcher,
    List<String> channels = const [_channelId],
  }) async {
    final socket = _RecordingRelaySocket();
    var clock = DateTime(2026, 10, 21);
    final session = RelaySessionNotifier(
      replayDelay: (_) async {},
      probeSequenceSupport: probe,
      now: () => clock = clock.add(const Duration(milliseconds: 1)),
    );
    final container = ProviderContainer(
      overrides: [
        relaySessionProvider.overrideWith(() => session),
        authProvider.overrideWith(_SignedOutAuthNotifier.new),
        relayConfigProvider.overrideWith(_FakeRelayConfigNotifier.new),
        relayInfoFetcherProvider.overrideWithValue(
          fetcher ?? (_) async => const RelayInfo(),
        ),
      ],
    );
    addTearDown(container.dispose);
    final listener = container.listen(relaySessionProvider, (_, _) {});
    addTearDown(listener.close);
    await container.read(authProvider.future);
    await Future<void>.delayed(Duration.zero);
    container.read(relaySessionProvider);
    session.debugAttachSocketForTest(socket);
    if (sequenced) session.debugEnableSequenceForTest();
    final harness = _Harness(session, socket);
    final subscribe = session.subscribe(
      NostrFilter(
        kinds: EventKind.channelEventKinds,
        tags: {'#h': channels},
        limit: 0,
      ),
      harness.delivered.add,
    );
    session.debugHandleMessage(['EOSE', 'l-1']);
    await subscribe;
    return harness;
  }

  void live(NostrEvent event, {required int sequence}) {
    session.debugHandleMessage([
      'EVENT',
      'l-1',
      event.toJson(),
      {'channel_sequence': sequence},
    ]);
    session.debugFlushEventBuffer();
  }

  List<List<dynamic>> get historyReqs => [
    for (final message in socket.messages)
      if (message.first == 'REQ' && (message[1] as String).startsWith('h-'))
        message,
  ];

  Future<void> answer(String subId, List<NostrEvent> events) async {
    for (final event in events) {
      session.debugHandleMessage(['EVENT', subId, event.toJson()]);
    }
    session.debugHandleMessage(['EOSE', subId]);
    await Future<void>.delayed(Duration.zero);
  }
}

NostrEvent _message(String id) => _event(id);

NostrEvent _pageBounds({
  required int after,
  required int last,
  required bool hasMore,
  required List<String> ids,
  String channel = _channelId,
}) => _event(
  'bounds-$channel-$after',
  kind: EventKind.channelWindowBounds,
  tags: [
    ['d', '$channel:after_seq:$after'],
    ['h', channel],
  ],
  content: jsonEncode({
    'order': 'seq',
    'after_seq': after,
    'last_seq': last,
    'has_more': hasMore,
    'sequences': [
      for (var i = 0; i < ids.length; i++) [ids[i], last - ids.length + 1 + i],
    ],
  }),
);

NostrEvent _event(
  String id, {
  int kind = EventKind.streamMessageV2,
  List<List<String>>? tags,
  String content = 'hello',
}) => NostrEvent(
  id: id,
  pubkey: 'alice',
  createdAt: 20,
  kind: kind,
  tags:
      tags ??
      [
        ['h', _channelId],
      ],
  content: content,
  sig: 'sig',
);

class _RecordingRelaySocket extends RelaySocket {
  _RecordingRelaySocket()
    : super(
        wsUrl: 'wss://relay.example',
        nsec: null,
        onMessage: (_) {},
        onConnected: () {},
        onDisconnected: (_) {},
      );

  final List<List<dynamic>> messages = [];

  @override
  void send(List<dynamic> payload) => messages.add(payload);

  @override
  void dispose() {}
}

class _SignedOutAuthNotifier extends AuthNotifier {
  @override
  Future<AuthState> build() async =>
      const AuthState(status: AuthStatus.unauthenticated);
}

class _FakeRelayConfigNotifier extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(baseUrl: _baseUrl, nsec: null);
}
