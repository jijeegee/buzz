import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel_head_cache.dart';
import 'package:kuzz/features/channels/channel_messages_provider.dart';
import 'package:kuzz/features/channels/window_anchor.dart';
import 'package:kuzz/shared/relay/relay.dart';

import 'window_relay_fixture.dart';

// Saved rows draw before the connection and the relay's window, and the
// store check never holds back the relay request
// (docs/message-sync.md#opening-a-room-from-the-store).

const _channelId = '11111111-1111-4111-8111-111111111111';

NostrEvent _row(int n) => fixtureRow(_channelId, (100000 - n) * 16, 1000 + n);

class _Session extends RelaySessionNotifier {
  _Session(this.rows, {this.connected = true});

  final List<NostrEvent> rows;
  final bool connected;
  final headGate = Completer<void>();
  final queries = <NostrFilter>[];
  final _listeners = <void Function(NostrEvent)>[];

  void emit(NostrEvent event) {
    for (final listener in List.of(_listeners)) {
      listener(event);
    }
  }

  @override
  SessionState build() => SessionState(
    status: connected ? SessionStatus.connected : SessionStatus.connecting,
  );

  void connect() => state = const SessionState(status: SessionStatus.connected);

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    queries.add(filters.single);
    await headGate.future;
    return serveChannelWindow(_channelId, rows, filters.single);
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
}

class _NoHeadCache extends ChannelHeadCache {
  _NoHeadCache([this.cached]) : super(() async => throw UnimplementedError());

  final List<NostrEvent>? cached;

  @override
  Future<List<NostrEvent>?> load(ChannelHeadScope scope, String id) async =>
      cached;

  @override
  Future<void> store(ChannelHeadScope scope, String id, List<NostrEvent> e) =>
      Future.value();
}

ProviderContainer _container(
  _Session session, {
  List<NostrEvent>? saved,
  Future<List<NostrEvent>?> Function()? certain,
  Future<List<NostrEvent>?> Function(NostrFilter filter)? paint,
  List<NostrEvent>? cached,
  ({int? from})? contiguous,
}) {
  final container = ProviderContainer(
    overrides: [
      relaySessionProvider.overrideWith(() => session),
      channelHeadScopeProvider.overrideWithValue(
        const ChannelHeadScope(relayUrl: 'wss://r', pubkey: 'me'),
      ),
      channelHeadCacheProvider.overrideWithValue(_NoHeadCache(cached)),
      paintableRoomHeadProvider.overrideWithValue(
        (_, _, filter) async =>
            paint?.call(filter) ??
            (saved == null
                ? null
                : _withContiguous(
                    serveChannelWindow(_channelId, saved, filter),
                    contiguous,
                  )),
      ),
      relayInfoProvider.overrideWith(
        (ref) async => RelayInfo(supportedExtensions: {newerWindowExtension}),
      ),
      storedRoomHeadProvider.overrideWithValue(
        (_, _, _) => certain?.call() ?? Future.value(null),
      ),
    ],
  );
  addTearDown(container.dispose);
  return container;
}

/// [events] with the store's gap-free bound in their bounds, as
/// MessageStore.paintableRoomHead answers.
List<NostrEvent> _withContiguous(
  List<NostrEvent> events,
  ({int? from})? contiguous,
) => [
  for (final event in events)
    if (contiguous != null && event.kind == EventKind.channelWindowBounds)
      NostrEvent(
        id: event.id,
        pubkey: event.pubkey,
        createdAt: event.createdAt,
        kind: event.kind,
        tags: event.tags,
        content: jsonEncode({
          ...jsonDecode(event.content) as Map<String, dynamic>,
          'contiguous_from': contiguous.from,
        }),
        sig: event.sig,
      )
    else
      event,
];

Future<void> _settle() async {
  for (var i = 0; i < 8; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

List<String> _shown(ProviderContainer container) => [
  for (final event
      in container.read(channelMessagesProvider(_channelId)).value ??
          const <NostrEvent>[])
    event.id,
];

void main() {
  test(
    'saved rows draw before the connection, then the relay replaces them',
    () async {
      final rows = [for (var n = 1; n <= 60; n++) _row(n)];
      final session = _Session(rows, connected: false);
      final container = _container(session, saved: rows.sublist(0, 55));
      container.listen(channelMessagesProvider(_channelId), (_, _) {});
      expect(
        container.read(channelSavedRowsCheckedProvider(_channelId)),
        false,
      );
      await _settle();

      final notifier = container.read(
        channelMessagesProvider(_channelId).notifier,
      );
      expect(container.read(channelSavedRowsCheckedProvider(_channelId)), true);
      expect(_shown(container).last, rows[54].id);
      expect(notifier.isShowingCachedHead, isTrue);
      expect(session.queries, isEmpty);

      session.connect();
      await _settle();
      // The relay request is out while the saved rows stay drawn.
      expect(session.queries, hasLength(1));
      expect(_shown(container).last, rows[54].id);

      session.headGate.complete();
      await _settle();
      expect(_shown(container).last, rows.last.id);
      expect(notifier.isShowingCachedHead, isFalse);
    },
  );

  test('nothing saved still marks the rows checked', () async {
    final session = _Session(const [], connected: false);
    final container = _container(session);
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    await _settle();
    expect(container.read(channelSavedRowsCheckedProvider(_channelId)), true);
    expect(_shown(container), isEmpty);
  });

  test('the relay request does not wait for the store check', () async {
    final rows = [for (var n = 1; n <= 60; n++) _row(n)];
    final session = _Session(rows);
    final storeCheck = Completer<List<NostrEvent>?>();
    final container = _container(session, certain: () => storeCheck.future);
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    await _settle();
    expect(session.queries, hasLength(1));

    // A certain window that answers first draws; the request already out is
    // its relay follow-up, not a second one.
    storeCheck.complete(
      serveChannelWindow(_channelId, rows.sublist(0, 58), session.queries[0]),
    );
    await _settle();
    expect(_shown(container).last, rows[57].id);
    session.headGate.complete();
    await _settle();
    expect(_shown(container).last, rows.last.id);
    expect(session.queries, hasLength(1));
  });

  ({String id, int createdAt}) at(NostrEvent row) =>
      (id: row.id, createdAt: row.createdAt);

  test(
    'a first unread row opens on the rows at hand only past an older row',
    () async {
      final rows = [for (var n = 1; n <= 60; n++) _row(n)];
      final session = _Session(rows, connected: false);
      final container = _container(session, saved: rows);
      container.listen(channelMessagesProvider(_channelId), (_, _) {});
      await _settle();
      final notifier = container.read(
        channelMessagesProvider(_channelId).notifier,
      );
      // The window holds the newest 50 rows: 11 to 60.
      expect(notifier.openOnRowsAtHand(at(rows[30]), unread: true), isTrue);
      expect(notifier.openOnRowsAtHand(at(rows[10])), isTrue);
      expect(notifier.openOnRowsAtHand(at(rows[10]), unread: true), isFalse);
      expect(notifier.openOnRowsAtHand(at(rows[5])), isFalse);
    },
  );

  test('saved rows open at a row only where they run without a gap', () async {
    final rows = [for (var n = 1; n <= 60; n++) _row(n)];
    final container = _container(
      _Session(rows, connected: false),
      saved: rows,
      contiguous: (from: rows[30].createdAt),
    );
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    await _settle();
    final notifier = container.read(
      channelMessagesProvider(_channelId).notifier,
    );
    expect(notifier.openOnRowsAtHand(at(rows[40])), isTrue);
    expect(notifier.openOnRowsAtHand(at(rows[20])), isFalse);
    // The row before a first unread one must be past the gap too.
    expect(notifier.openOnRowsAtHand(at(rows[30]), unread: true), isFalse);
    expect(notifier.openOnRowsAtHand(at(rows[31]), unread: true), isTrue);

    final unvouched = _container(
      _Session(rows, connected: false),
      saved: rows,
      contiguous: (from: null),
    );
    unvouched.listen(channelMessagesProvider(_channelId), (_, _) {});
    await _settle();
    expect(
      unvouched
          .read(channelMessagesProvider(_channelId).notifier)
          .openOnRowsAtHand(at(rows[59])),
      isFalse,
    );
  });

  test(
    'a saved row opened at stays when the relay newest page lacks it',
    () async {
      final rows = [for (var n = 1; n <= 200; n++) _row(n)];
      final session = _Session(rows);
      final container = _container(session, saved: rows.sublist(0, 60));
      container.listen(channelMessagesProvider(_channelId), (_, _) {});
      await _settle();
      final notifier = container.read(
        channelMessagesProvider(_channelId).notifier,
      );
      expect(notifier.openOnRowsAtHand(at(rows[20])), isTrue);

      session.headGate.complete();
      await _settle();
      // The window around the row, not the unrelated newest page.
      expect(_shown(container), contains(rows[20].id));
      expect(_shown(container), isNot(contains(rows.last.id)));
      expect(notifier.isInMiddleWindow, isTrue);
      expect(notifier.isShowingCachedHead, isFalse);
      expect(session.queries, hasLength(3));
    },
  );

  test('a live row between the cached and the stored draw stays', () async {
    final rows = [for (var n = 1; n <= 60; n++) _row(n)];
    final session = _Session(rows);
    final stored = Completer<List<NostrEvent>?>();
    final container = _container(
      session,
      cached: serveChannelWindow(
        _channelId,
        rows.sublist(0, 50),
        NostrFilter(kinds: const [9], limit: 50),
      ),
      paint: (_) => stored.future,
    );
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    await _settle();
    expect(_shown(container).last, rows[49].id);

    session.emit(rows[50]);
    await _settle();
    expect(_shown(container).last, rows[50].id);

    stored.complete(
      serveChannelWindow(
        _channelId,
        rows.sublist(0, 55),
        NostrFilter(kinds: const [9], limit: 50),
      ),
    );
    await _settle();
    expect(_shown(container), contains(rows[50].id));
    expect(container.read(channelSavedRowsCheckedProvider(_channelId)), true);
  });
}
