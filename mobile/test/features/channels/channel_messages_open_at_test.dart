import 'package:buzz/features/channels/channel_messages_provider.dart';
import 'package:buzz/features/channels/window_anchor.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'window_relay_fixture.dart';

const _channelId = '11111111-1111-4111-8111-111111111111';

/// Row [n] of the channel, in timeline order: three rows a second, and within
/// a second the newer row has the lower id, as the relay orders them.
NostrEvent _row(int n) =>
    fixtureRow(_channelId, (100000 - n) * 16, 1000 + n ~/ 3);

List<NostrEvent> _rows(int from, int to) => [
  for (var n = from; n <= to; n++) _row(n),
];

/// Serves NIP-CW channel windows over [rows] like the relay's keyset SQL.
class _WindowRelaySession extends RelaySessionNotifier {
  _WindowRelaySession(this.rows);

  List<NostrEvent> rows;
  final queries = <NostrFilter>[];
  final _listeners = <void Function(NostrEvent)>[];

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  void setConnected(bool connected) {
    state = SessionState(
      status: connected ? SessionStatus.connected : SessionStatus.disconnected,
    );
  }

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    final filter = filters.single;
    queries.add(filter);
    return serveChannelWindow(_channelId, rows, filter);
  }

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async => const [];

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    _listeners.add(onEvent);
    return () => _listeners.remove(onEvent);
  }

  void emit(NostrEvent event) {
    for (final listener in List.of(_listeners)) {
      listener(event);
    }
  }
}

bool _isHead(NostrFilter filter) =>
    filter.until == null && filter.extensions['after_id'] == null;

ProviderContainer _container(
  _WindowRelaySession session, {
  Future<RelayInfo?> Function()? relayInfo,
}) {
  final container = ProviderContainer(
    overrides: [
      relaySessionProvider.overrideWith(() => session),
      relayInfoProvider.overrideWith(
        (ref) =>
            relayInfo?.call() ??
            Future.value(
              const RelayInfo(supportedExtensions: {newerWindowExtension}),
            ),
      ),
    ],
  );
  addTearDown(container.dispose);
  return container;
}

Future<void> _settle() async {
  for (var i = 0; i < 5; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

List<String> _shown(ProviderContainer container) => container
    .read(channelMessagesProvider(_channelId))
    .value!
    .map((event) => event.id)
    .toList();

ChannelMessagesNotifier _notifier(ProviderContainer container) =>
    container.read(channelMessagesProvider(_channelId).notifier);

Future<void> _pageToPresent(ChannelMessagesNotifier notifier) async {
  for (var i = 0; notifier.hasNewer; i++) {
    expect(i, lessThan(100), reason: 'newer paging must terminate');
    expect(await notifier.fetchNewer(), isTrue);
  }
}

void main() {
  test(
    'opens at an old message and pages down to the newest without gaps',
    () async {
      final rows = _rows(1, 300);
      final session = _WindowRelaySession(rows);
      final container = _container(session);
      container.listen(channelMessagesProvider(_channelId), (_, _) {});
      final notifier = _notifier(container);
      final anchor = rows[99];

      // Called while the newest page is still loading: it waits for it.
      expect(
        await notifier.openAt(createdAt: anchor.createdAt, eventId: anchor.id),
        isTrue,
      );
      expect(notifier.isInMiddleWindow, isTrue);
      expect(notifier.anchorId, anchor.id);
      expect(notifier.windowGeneration, 1);
      expect(notifier.hasNewer, isTrue);
      final preceding = windowAnchorCursor(
        createdAt: anchor.createdAt,
        eventId: anchor.id,
      ).eventId;
      final opening = session.queries.skip(1).toList();
      expect(opening.length, 2);
      expect(opening[0].until, anchor.createdAt);
      expect(opening[0].extensions['before_id'], preceding);
      expect(opening[1].since, anchor.createdAt);
      expect(opening[1].until, isNull);
      expect(opening[1].extensions['after_id'], preceding);
      expect(opening[1].extensions['before_id'], isNull);

      // 50 back from the anchor (inclusive) and 50 after it.
      expect(_shown(container), rows.sublist(50, 150).map((event) => event.id));

      await _pageToPresent(notifier);
      final shown = _shown(container);
      expect(shown, rows.skip(50).map((event) => event.id));
      expect(shown.toSet().length, shown.length);
      // Each newer page continued from the last row of the one before.
      final newer = session.queries
          .where((filter) => filter.extensions['after_id'] != null)
          .toList();
      expect(newer.length, 4);
      expect(newer[1].since, rows[149].createdAt);
      expect(newer[1].extensions['after_id'], rows[149].id);

      // Older paging still works from the anchor's page.
      while (!notifier.reachedOldest) {
        await notifier.fetchOlder();
      }
      expect(_shown(container), rows.map((event) => event.id));
    },
  );

  test(
    'a read marker opens after its second: older <= ts, newer > ts',
    () async {
      final rows = _rows(1, 200);
      final session = _WindowRelaySession(rows);
      final container = _container(session);
      container.listen(channelMessagesProvider(_channelId), (_, _) {});
      final notifier = _notifier(container);
      await _settle();

      const ts = 1040; // rows 120, 121, 122
      expect(await notifier.openAt(createdAt: ts), isTrue);
      expect(notifier.anchorId, isNull);
      final opening = session.queries.skip(1).toList();
      expect(opening[0].extensions['before_id'], zeroEventId);
      expect(opening[1].extensions['after_id'], zeroEventId);
      final shown = _shown(container);
      // The last row at ts is the newest read row; the first unread follows.
      final split = shown.indexOf(_row(122).id);
      expect(shown[split + 1], _row(123).id);
      expect(shown.sublist(0, split + 1).length, 50);
      expect(shown.sublist(split + 1).length, 50);
    },
  );

  test('live rows wait past the gap and show once the gap closes', () async {
    final rows = _rows(1, 200);
    final session = _WindowRelaySession(rows);
    final container = _container(session);
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    final notifier = _notifier(container);
    await _settle();
    expect(
      await notifier.openAt(createdAt: rows[9].createdAt, eventId: rows[9].id),
      isTrue,
    );

    final live = _row(201);
    session.emit(live);
    expect(_shown(container), isNot(contains(live.id)));
    expect(notifier.heldLiveCount, 1);

    await _pageToPresent(notifier);
    expect(notifier.heldLiveCount, 0);
    final shown = _shown(container);
    expect(shown.last, live.id);
    expect(shown.where((id) => id == live.id).length, 1);

    // Once connected, live rows show at once again.
    final next = _row(202);
    session.emit(next);
    expect(_shown(container).last, next.id);
  });

  test('a reconnect keeps the middle window and pages on from it', () async {
    final rows = _rows(1, 300);
    final session = _WindowRelaySession(rows);
    final container = _container(session);
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    final notifier = _notifier(container);
    await _settle();
    expect(
      await notifier.openAt(
        createdAt: rows[49].createdAt,
        eventId: rows[49].id,
      ),
      isTrue,
    );
    expect(await notifier.fetchNewer(), isTrue);
    final before = _shown(container);

    session.setConnected(false);
    await _settle();
    // Rows posted while the socket was down.
    session.rows = [...rows, ..._rows(301, 320)];
    final queriesBefore = session.queries.length;
    session.setConnected(true);
    await _settle();

    final reconnect = session.queries.skip(queriesBefore).toList();
    expect(reconnect.where(_isHead), isEmpty);
    expect(reconnect.single.extensions['after_id'], rows[149].id);
    expect(notifier.isInMiddleWindow, isTrue);
    expect(notifier.windowGeneration, 1);
    final after = _shown(container);
    expect(after.take(before.length), before);

    await _pageToPresent(notifier);
    expect(_shown(container), session.rows.map((event) => event.id));
  });

  test(
    'a reconnect after reaching the present pages the missed rows',
    () async {
      final rows = _rows(1, 120);
      final session = _WindowRelaySession(rows);
      final container = _container(session);
      container.listen(channelMessagesProvider(_channelId), (_, _) {});
      final notifier = _notifier(container);
      await _settle();
      await notifier.openAt(
        createdAt: rows[79].createdAt,
        eventId: rows[79].id,
      );
      await _pageToPresent(notifier);

      session.setConnected(false);
      await _settle();
      session.rows = [...rows, ..._rows(121, 125)];
      session.setConnected(true);
      await _settle();

      expect(notifier.hasNewer, isFalse);
      expect(_shown(container), session.rows.skip(30).map((event) => event.id));
    },
  );

  test('without nip-cw-newer openAt keeps the newest window', () async {
    final rows = _rows(1, 120);
    final session = _WindowRelaySession(rows);
    final container = _container(
      session,
      relayInfo: () async => const RelayInfo(supportedExtensions: {'nip-pl'}),
    );
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    final notifier = _notifier(container);
    await _settle();
    final newest = _shown(container);

    expect(
      await notifier.openAt(createdAt: rows[5].createdAt, eventId: rows[5].id),
      isFalse,
    );
    expect(session.queries.length, 1);
    expect(notifier.isInMiddleWindow, isFalse);
    expect(notifier.hasNewer, isFalse);
    expect(notifier.windowGeneration, 0);
    expect(_shown(container), newest);
    expect(await notifier.fetchNewer(), isFalse);
    expect(await notifier.jumpToLatest(), isFalse);
    // Older paging is untouched.
    expect(await notifier.fetchOlder(), isTrue);
  });

  test('an unreadable relay document falls back and is retried', () async {
    var reads = 0;
    final session = _WindowRelaySession(_rows(1, 120));
    final container = _container(
      session,
      relayInfo: () async {
        reads++;
        return null;
      },
    );
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    final notifier = _notifier(container);
    await _settle();

    expect(await notifier.openAt(createdAt: 1010), isFalse);
    expect(await notifier.openAt(createdAt: 1010), isFalse);
    expect(reads, 2);
    expect(session.queries.length, 1);
  });

  test('jumpToLatest leaves the middle window with held rows shown', () async {
    final rows = _rows(1, 300);
    final session = _WindowRelaySession(rows);
    final container = _container(session);
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    final notifier = _notifier(container);
    await _settle();
    await notifier.openAt(createdAt: rows[9].createdAt, eventId: rows[9].id);
    final live = _row(301);
    session.emit(live);

    expect(await notifier.jumpToLatest(), isTrue);
    expect(session.queries.last.until, isNull);
    expect(_isHead(session.queries.last), isTrue);
    expect(notifier.isInMiddleWindow, isFalse);
    expect(notifier.hasNewer, isFalse);
    expect(notifier.windowGeneration, 2);
    expect(
      _shown(container),
      [...rows.skip(250), live].map((event) => event.id),
    );
    expect(await notifier.jumpToLatest(), isFalse);
  });
}
