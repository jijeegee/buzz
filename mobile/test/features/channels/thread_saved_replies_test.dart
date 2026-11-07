import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'package:kuzz/features/channels/channel_preload.dart';
import 'package:kuzz/features/channels/relay_cut.dart';
import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:kuzz/features/channels/thread_saved_replies.dart';
import 'package:kuzz/features/channels/thread_window.dart';
import 'package:kuzz/features/channels/window_anchor.dart';
import 'package:kuzz/shared/push/push_inbox_events.dart';
import 'package:kuzz/shared/relay/relay.dart';

import 'window_relay_fixture.dart';

const _channel = '11111111-1111-4111-8111-111111111111';
final _root = 'f' * 64;
final _args = ThreadRepliesArgs(channelId: _channel, rootId: _root);

class _Offline extends RelaySessionNotifier {
  @override
  SessionState build() =>
      const SessionState(status: SessionStatus.disconnected);
}

NostrEvent _reply(String id, int at, {String content = ''}) => NostrEvent(
  id: id,
  pubkey: 'b' * 64,
  createdAt: at,
  kind: 9,
  tags: [
    ['h', _channel],
    ['e', _root, '', 'reply'],
  ],
  content: content.isEmpty ? id : content,
  sig: '',
);

/// Serves a thread's window over [rows].
class _Relay extends RelaySessionNotifier {
  _Relay(this.rows);

  final List<NostrEvent> rows;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async => filters.single.extensions['thread_window'] == true
      ? serveThreadWindow(_channel, _root, rows, filters.single)
      : const [];
}

/// The device store's replies of the thread.
class _Store {
  final rows = <NostrEvent>[];
  final pushedAt = <String, int?>{};

  void push(NostrEvent row, {int? fetchedAt}) {
    rows.add(row);
    pushedAt[row.id] = fetchedAt ?? relayAskTime();
  }

  List<NostrEvent> read() => [
    for (final event in serveThreadWindow(
      _channel,
      _root,
      rows,
      NostrFilter(kinds: const [], limit: 50),
    ))
      if (event.kind == threadWindowBoundsKind)
        NostrEvent(
          id: event.id,
          pubkey: event.pubkey,
          createdAt: event.createdAt,
          kind: event.kind,
          tags: event.tags,
          content: jsonEncode({
            ...jsonDecode(event.content) as Map<String, dynamic>,
            if (pushedAt.isNotEmpty) 'pushed_at': pushedAt,
          }),
          sig: '',
        )
      else
        event,
  ];
}

String _hex(int n) => fixtureId(n);

ProviderContainer _threadContainer(_Relay relay, _Store store) {
  final container = ProviderContainer(
    overrides: [
      relaySessionProvider.overrideWith(() => relay),
      paintableThreadHeadProvider.overrideWithValue(
        (_, _) async => store.read(),
      ),
      storedThreadHeadProvider.overrideWithValue((_, _) async => null),
      relayInfoProvider.overrideWith(
        (ref) async => RelayInfo(supportedExtensions: {newerWindowExtension}),
      ),
    ],
  );
  addTearDown(container.dispose);
  return container;
}

/// A thread holding the relay's page, and the replies shown past it.
Future<void> _holdRelayPage(ProviderContainer container) async {
  container.listen(threadRepliesProvider(_args), (_, _) {});
  await container.read(threadRepliesProvider(_args).future);
}

Future<List<String>> _newer(ProviderContainer container) async {
  final listening = container.listen(
    storedNewerRepliesProvider(_args),
    (_, _) {},
  );
  await container.read(storedThreadRepliesProvider(_args).future);
  await pumpEventQueue();
  final ids = [for (final reply in listening.read()) reply.id];
  listening.close();
  await pumpEventQueue();
  return ids;
}

/// A thread's saved replies read the store again after a push inbox drain
/// wrote to it (docs/message-sync.md#push-inbox).
void main() {
  test('a drain re-reads the stored replies, merged with the preload, '
      'without showing loading again', () async {
    var stored = [_reply('r1', 10)];
    var reads = 0;
    final container = ProviderContainer(
      overrides: [
        relaySessionProvider.overrideWith(_Offline.new),
        paintableThreadHeadProvider.overrideWithValue((_, _) async {
          reads++;
          return serveThreadWindow(
            _channel,
            _root,
            stored,
            NostrFilter(kinds: const [], limit: 50),
          );
        }),
        preloadedThreadRepliesProvider.overrideWith(
          (ref, args) => [_reply('r1', 10, content: 'preloaded')],
        ),
      ],
    );
    addTearDown(container.dispose);
    final loading = <bool>[];
    container.listen(
      savedRepliesLoadingProvider(_args),
      (_, next) => loading.add(next),
      fireImmediately: true,
    );
    container.listen(savedThreadRepliesProvider(_args), (_, _) {});
    await container.read(storedThreadRepliesProvider(_args).future);
    expect(
      container.read(savedThreadRepliesProvider(_args)).single.content,
      'preloaded',
    );

    stored = [_reply('r1', 10), _reply('r2', 20)];
    container.read(pushInboxRevisionProvider.notifier).bump();
    await container.read(storedThreadRepliesProvider(_args).future);
    await pumpEventQueue();

    expect(reads, 2);
    final replies = container.read(savedThreadRepliesProvider(_args));
    expect(replies.map((reply) => reply.id), ['r1', 'r2']);
    expect(replies.first.content, 'preloaded', reason: 'the preload wins');
    expect(loading, [true, false], reason: 'the re-read keeps its replies');
  });
  test(
    'over a preload, only stored replies newer than its newest are added',
    () async {
      final container = ProviderContainer(
        overrides: [
          relaySessionProvider.overrideWith(_Offline.new),
          paintableThreadHeadProvider.overrideWithValue(
            (_, _) async => serveThreadWindow(
              _channel,
              _root,
              // r2 is gone from the relay (deleted), but the store kept it.
              [_reply('r1', 10), _reply('r2', 20), _reply('r4', 40)],
              NostrFilter(kinds: const [], limit: 50),
            ),
          ),
          preloadedThreadRepliesProvider.overrideWith(
            (ref, args) => [_reply('r1', 10), _reply('r3', 30)],
          ),
        ],
      );
      addTearDown(container.dispose);
      container.listen(savedThreadRepliesProvider(_args), (_, _) {});
      await container.read(storedThreadRepliesProvider(_args).future);
      expect(
        container.read(savedThreadRepliesProvider(_args)).map((r) => r.id),
        ['r1', 'r3', 'r4'],
      );
    },
  );

  test('reactions in a preload ride along and never move its newest', () {
    // Newer than every reply: only replies set what counts as newer.
    final reaction = NostrEvent(
      id: 'x',
      pubkey: 'c' * 64,
      createdAt: 500,
      kind: EventKind.reaction,
      tags: const [
        ['e', 'r1'],
      ],
      content: '+',
      sig: '',
    );
    ProviderContainer open(List<NostrEvent> preloaded) {
      final container = ProviderContainer(
        overrides: [
          relaySessionProvider.overrideWith(_Offline.new),
          paintableThreadHeadProvider.overrideWithValue(
            (_, _) async => serveThreadWindow(_channel, _root, [
              _reply('r1', 10),
              _reply('r4', 40),
            ], NostrFilter(kinds: const [], limit: 50)),
          ),
          preloadedThreadRepliesProvider.overrideWith((ref, args) => preloaded),
        ],
      );
      addTearDown(container.dispose);
      container.listen(savedThreadRepliesProvider(_args), (_, _) {});
      return container;
    }

    Future<List<String>> saved(ProviderContainer container) async {
      await container.read(storedThreadRepliesProvider(_args).future);
      return [
        for (final e in container.read(savedThreadRepliesProvider(_args))) e.id,
      ];
    }

    return Future.wait([
      saved(
        open([_reply('r1', 10), _reply('r3', 30), reaction]),
      ).then((ids) => expect(ids, ['r1', 'r3', 'r4', 'x'])),
      // A preload of reactions alone: the stored replies stand, with them.
      saved(open([reaction])).then((ids) => expect(ids, ['r1', 'r4', 'x'])),
    ]);
  });

  group('stored replies past the relay cut', () {
    final relayRows = [for (var n = 1; n <= 3; n++) _reply(_hex(n), 100 + n)];

    test('a pushed reply fetched since the ask joins a held thread', () async {
      final store = _Store()..rows.addAll(relayRows);
      final container = _threadContainer(_Relay(relayRows), store);
      await _holdRelayPage(container);
      expect(await _newer(container), isEmpty);

      final pushed = _reply(_hex(4), 104);
      store.push(pushed);
      container.read(pushInboxRevisionProvider.notifier).bump();
      expect(await _newer(container), [pushed.id]);
    });

    test('a thread reopened reads the store without a drain', () async {
      final store = _Store()..rows.addAll(relayRows);
      final container = _threadContainer(_Relay(relayRows), store);
      await _holdRelayPage(container);
      expect(await _newer(container), isEmpty);

      // Written while the page was closed; the retained thread comes back.
      final pushed = _reply(_hex(4), 104);
      store.push(pushed);
      expect(await _newer(container), [pushed.id]);
    });

    test('leaves replies at or before the cut', () async {
      final store = _Store()..rows.addAll(relayRows);
      final container = _threadContainer(_Relay(relayRows), store);
      await _holdRelayPage(container);

      store.push(_reply(_hex(4), 104), fetchedAt: relayAskTime() - 100);
      store.rows.add(_reply(_hex(5), 105));
      expect(await _newer(container), isEmpty);
    });

    test('counts a reply by its fetch, any other by its time', () async {
      final store = _Store()..rows.addAll(relayRows);
      final container = _threadContainer(_Relay(relayRows), store);
      await _holdRelayPage(container);

      final behind = _reply(_hex(4), 50);
      final ahead = _reply(_hex(5), relayAskTime() + 100);
      store.push(behind);
      store.rows.add(ahead);
      expect(await _newer(container), unorderedEquals([behind.id, ahead.id]));
    });

    test('a certain window from the store rules on nothing', () async {
      final store = _Store()..rows.addAll(relayRows);
      final container = _threadContainer(_Relay(relayRows), store);
      final window = container.read(threadWindowProvider(_args).notifier);
      window.foldNewest(
        parseThreadWindowPage(
          store.read(),
          channelId: _channel,
          rootId: _root,
        )!,
      );

      expect(window.relayCut, isNull);
      final saved = _reply(_hex(4), 104);
      final older = _reply(_hex(5), 102);
      store.rows.addAll([saved, older]);
      expect(await _newer(container), [saved.id]);
    });

    test('every relay page moves the cut, an older page does not', () async {
      final rows = [for (var n = 1; n <= 200; n++) _reply(_hex(n), 100 + n)];
      final container = _threadContainer(_Relay(rows), _Store());
      await _holdRelayPage(container);
      final window = container.read(threadWindowProvider(_args).notifier);
      int newest() => window.relayCut!.newest;
      expect(newest(), 300);

      await window.loadOlder();
      expect(newest(), 300);

      // Each kind of page moves the ask time.
      Future<void> moves(Future<bool> Function() page) async {
        final before = window.relayCut!.askedAt;
        await Future<void>.delayed(const Duration(milliseconds: 1100));
        expect(await page(), isTrue);
        expect(window.relayCut!.askedAt, greaterThan(before));
      }

      await moves(
        () =>
            window.openAt(createdAt: rows[20].createdAt, eventId: rows[20].id),
      );
      await moves(window.loadNewer);
      await moves(window.jumpToLatest);
    });

    test('while newer pages remain, stored replies wait', () async {
      final rows = [for (var n = 1; n <= 200; n++) _reply(_hex(n), 100 + n)];
      final store = _Store();
      final container = _threadContainer(_Relay(rows), store);
      await _holdRelayPage(container);
      final window = container.read(threadWindowProvider(_args).notifier);
      await window.openAt(createdAt: rows[20].createdAt, eventId: rows[20].id);

      store.push(_reply(_hex(300), 400));
      expect(await _newer(container), isEmpty);
    });
  });
}
