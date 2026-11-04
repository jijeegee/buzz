import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import 'package:kuzz/features/channels/channel_head_cache.dart';
import 'package:kuzz/features/channels/channel_messages_provider.dart';
import 'package:kuzz/features/channels/pending_local_messages_provider.dart';
import 'package:kuzz/features/channels/relay_cut.dart';
import 'package:kuzz/features/channels/window_anchor.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/push/push_inbox_drain.dart';
import 'package:kuzz/shared/push/push_inbox_events.dart';
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
  var headGate = Completer<void>();

  /// Overlays the relay adds to every window (thread summaries).
  final overlays = <NostrEvent>[];
  final queries = <NostrFilter>[];
  final fetches = <NostrFilter>[];
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

  void disconnect() =>
      state = const SessionState(status: SessionStatus.disconnected);

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    queries.add(filters.single);
    await headGate.future;
    return [
      ...overlays,
      ...serveChannelWindow(_channelId, rows, filters.single),
    ];
  }

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    fetches.add(filter);
    return const [];
  }

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
    void Function()? onGap,
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

/// [events] with the extras a device store adds to its bounds.
List<NostrEvent> _withBounds(
  List<NostrEvent> events, {
  int? contiguousFrom,
  Map<String, int?> pushedAt = const {},
}) => [
  for (final event in events)
    if (event.kind == EventKind.channelWindowBounds)
      NostrEvent(
        id: event.id,
        pubkey: event.pubkey,
        createdAt: event.createdAt,
        kind: event.kind,
        tags: event.tags,
        content: jsonEncode({
          ...jsonDecode(event.content) as Map<String, dynamic>,
          'contiguous_from': ?contiguousFrom,
          if (pushedAt.isNotEmpty) 'pushed_at': pushedAt,
        }),
        sig: event.sig,
      )
    else
      event,
];

/// The device store's rows for one room.
class _Store {
  _Store(this.rows);

  final List<NostrEvent> rows;
  final pushedAt = <String, int?>{};
  final overlays = <NostrEvent>[];
  Completer<void>? gate;
  var paints = 0;

  void push(NostrEvent row, {int? fetchedAt}) {
    rows.add(row);
    pushedAt[row.id] = fetchedAt ?? relayAskTime();
  }

  Future<List<NostrEvent>?> read(NostrFilter filter) async {
    paints++;
    await gate?.future;
    return [
      ...overlays,
      ..._withBounds(
        serveChannelWindow(_channelId, rows, filter),
        pushedAt: pushedAt,
      ),
    ];
  }
}

ProviderContainer _storeContainer(
  _Session session,
  _Store store, {
  Future<List<NostrEvent>?> Function()? certain,
}) => _container(session, paint: store.read, certain: certain);

class _KeptRoom {
  _KeptRoom(this.rows, this.session, this.store) {
    container = _storeContainer(
      session,
      store,
      certain: () async {
        return certain?.call();
      },
    );
    listening = container.listen(
      channelMessagesProvider(_channelId),
      (_, _) {},
    );
  }

  final List<NostrEvent> rows;
  final _Session session;
  final _Store store;
  late final ProviderContainer container;
  late final ProviderSubscription<Object?> listening;
  Future<List<NostrEvent>?> Function()? certain;
  late int shownAtStart;

  ChannelMessagesNotifier get notifier =>
      container.read(channelMessagesProvider(_channelId).notifier);

  void bump() => container.read(pushInboxRevisionProvider.notifier).bump();

  /// The relay's window again, over a reconnect.
  Future<void> relayAgain() async {
    session.disconnect();
    await _settle();
    session.connect();
    await _settle();
  }
}

/// A room whose relay window landed and stays listened to.
Future<_KeptRoom> _keptRoom({int count = 60}) async {
  final rows = [for (var n = 1; n <= count; n++) _row(n)];
  final session = _Session(rows)..headGate.complete();
  final room = _KeptRoom(rows, session, _Store(rows.toList()));
  await _settle();
  room.shownAtStart = _shown(room.container).length;
  expect(_shown(room.container).last, rows.last.id);
  return room;
}

NostrEvent _rowAfter(NostrEvent row, [int seconds = 1]) =>
    fixtureRow(_channelId, 6 + seconds, row.createdAt + seconds);

NostrEvent _summary(
  String root, {
  required int replies,
  required int descendants,
  required int last,
  required String who,
}) => NostrEvent(
  id: 'summary-$root-$last',
  pubkey: 'relay',
  createdAt: last,
  kind: EventKind.channelThreadSummary,
  tags: [
    ['h', _channelId],
    ['e', root],
  ],
  content: jsonEncode({
    'reply_count': replies,
    'descendant_count': descendants,
    'last_reply_at': last,
    'participants': [who],
  }),
  sig: '',
);

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

  test('a push inbox drain redraws the saved rows until the relay window '
      'lands', () async {
    final rows = [for (var n = 1; n <= 60; n++) _row(n)];
    final session = _Session(rows, connected: false);
    var stored = rows.sublist(0, 55);
    var paints = 0;
    final container = _container(
      session,
      paint: (filter) async {
        paints++;
        return serveChannelWindow(_channelId, stored, filter);
      },
    );
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    await _settle();
    expect(_shown(container).last, rows[54].id);

    // The drain wrote a newer row into the store.
    stored = rows.sublist(0, 56);
    container.read(pushInboxRevisionProvider.notifier).bump();
    await _settle();
    expect(paints, 2);
    expect(_shown(container).last, rows[55].id);

    session.connect();
    session.headGate.complete();
    await _settle();
    expect(_shown(container).last, rows.last.id);

    // Once the relay's window is drawn, a drain still reads the store, and
    // what the relay answered for stays as it is.
    final shown = _shown(container);
    container.read(pushInboxRevisionProvider.notifier).bump();
    await _settle();
    expect(paints, 3);
    expect(_shown(container), shown);
  });

  test('a first store read answering after a redraw does not draw over '
      'it, nor does the cached head', () async {
    final rows = [for (var n = 1; n <= 60; n++) _row(n)];
    final session = _Session(rows, connected: false);
    final firstRead = Completer<List<NostrEvent>?>();
    var paints = 0;
    final container = _container(
      session,
      cached: serveChannelWindow(
        _channelId,
        rows.sublist(0, 50),
        NostrFilter(kinds: const [], limit: 50),
      ),
      paint: (filter) {
        paints++;
        if (paints == 1) return firstRead.future;
        return Future.value(
          serveChannelWindow(_channelId, rows.sublist(0, 56), filter),
        );
      },
    );
    final listening = container.listen(
      channelMessagesProvider(_channelId),
      (_, _) {},
    );
    await _settle();
    expect(_shown(container).last, rows[49].id, reason: 'the cached head');

    // A drain lands while the first store read is still out.
    container.read(pushInboxRevisionProvider.notifier).bump();
    await _settle();
    expect(_shown(container).last, rows[55].id);

    firstRead.complete(
      serveChannelWindow(
        _channelId,
        rows.sublist(0, 55),
        NostrFilter(kinds: const [], limit: 50),
      ),
    );
    await _settle();
    expect(_shown(container).last, rows[55].id, reason: 'older read ignored');

    // A room nobody watches does not read the store on a drain.
    listening.close();
    await _settle();
    container.read(pushInboxRevisionProvider.notifier).bump();
    await _settle();
    expect(paints, 2);
  });

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
    'an unread row opens on saved rows; a relay row only past an older one',
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
      // Unread and saved: the relay fills above it.
      expect(notifier.openOnRowsAtHand(at(rows[10]), unread: true), isTrue);
      expect(notifier.openOnRowsAtHand(at(rows[5])), isFalse);

      session.connect();
      session.headGate.complete();
      await _settle();
      // Held from the relay, the row before it is what a divider needs.
      expect(notifier.openOnRowsAtHand(at(rows[10]), unread: true), isFalse);
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
    // An unread row opens where it is whatever lies above it.
    expect(notifier.openOnRowsAtHand(at(rows[30]), unread: true), isTrue);
    expect(notifier.openOnRowsAtHand(at(rows[20]), unread: true), isTrue);

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

  test('a pushed row opens where it is past a gap, without unread', () async {
    final rows = [for (var n = 1; n <= 60; n++) _row(n)];
    final container = _container(
      _Session(rows, connected: false),
      paint: (filter) async => _withBounds(
        serveChannelWindow(_channelId, rows, filter),
        contiguousFrom: rows[30].createdAt,
        pushedAt: {rows[20].id: 5},
      ),
    );
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    await _settle();
    final notifier = container.read(
      channelMessagesProvider(_channelId).notifier,
    );
    expect(notifier.openOnRowsAtHand(at(rows[20])), isTrue);
    expect(notifier.openOnRowsAtHand(at(rows[19])), isFalse);
  });

  test('a row opened at before the connection survives it', () async {
    final rows = [for (var n = 1; n <= 200; n++) _row(n)];
    final session = _Session(rows, connected: false);
    final container = _container(session, saved: rows.sublist(0, 60));
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    await _settle();
    final notifier = container.read(
      channelMessagesProvider(_channelId).notifier,
    );
    expect(notifier.openOnRowsAtHand(at(rows[20]), unread: true), isTrue);

    session.connect();
    session.headGate.complete();
    await _settle();
    expect(_shown(container), contains(rows[20].id));
    expect(_shown(container), isNot(contains(rows.last.id)));
    expect(notifier.isInMiddleWindow, isTrue);
  });

  group('a store read after the relay answered', () {
    test('adds a pushed row fetched since the ask', () async {
      final room = await _keptRoom();
      final row = _rowAfter(room.rows.last);
      room.store.push(row);
      room.bump();
      await _settle();

      expect(_shown(room.container).last, row.id);
      expect(room.store.paints, 2);
      expect(room.notifier.isShowingCachedHead, isFalse);
    });

    test('leaves rows at or before the cut', () async {
      final room = await _keptRoom();
      final fetchedBefore = _rowAfter(room.rows.last);
      final saved = _rowAfter(room.rows.last, 2);
      room.store.push(fetchedBefore, fetchedAt: relayAskTime() - 100);
      room.store.rows.add(saved);
      room.bump();
      await _settle();

      expect(_shown(room.container), hasLength(room.shownAtStart));
    });

    test('counts a pushed row by its fetch, any other by its time', () async {
      final room = await _keptRoom();
      // A sender whose clock runs behind.
      final behind = fixtureRow(_channelId, 7, room.rows[30].createdAt);
      final ahead = fixtureRow(_channelId, 8, relayAskTime() + 100);
      room.store.push(behind);
      room.store.rows.add(ahead);
      room.bump();
      await _settle();

      expect(_shown(room.container), containsAll([behind.id, ahead.id]));
    });

    test('leaves a row deleted live', () async {
      final room = await _keptRoom();
      final row = _rowAfter(room.rows.last);
      room.session.emit(
        NostrEvent(
          id: 'd' * 64,
          pubkey: row.pubkey,
          createdAt: row.createdAt,
          kind: EventKind.deletion,
          tags: [
            ['h', _channelId],
            ['e', row.id],
          ],
          content: '',
          sig: '',
        ),
      );
      await _settle();
      room.store.push(row);
      room.bump();
      await _settle();
      expect(_shown(room.container), isNot(contains(row.id)));
    });

    test('a link to the row still asks the relay for it', () async {
      final room = await _keptRoom();
      final row = _rowAfter(room.rows.last);
      room.store.push(row);
      room.bump();
      await _settle();
      await room.notifier.loadEventsById([row.id]);
      await _settle();
      expect(room.session.fetches, hasLength(1));
    });

    test('a row the relay also answered with shows once', () async {
      final rows = [for (var n = 1; n <= 60; n++) _row(n)];
      final session = _Session(rows, connected: false);
      final store = _Store(rows.sublist(0, 59));
      final container = _storeContainer(session, store);
      container.listen(channelMessagesProvider(_channelId), (_, _) {});
      await _settle();

      // The drain draws first; the relay's window then holds the same row.
      store.push(rows.last);
      container.read(pushInboxRevisionProvider.notifier).bump();
      await _settle();
      session.connect();
      session.headGate.complete();
      await _settle();
      container.read(pushInboxRevisionProvider.notifier).bump();
      await _settle();

      expect(_shown(container).where((id) => id == rows.last.id), hasLength(1));
    });

    test('a relay page drops a row it lacks, for good', () async {
      final room = await _keptRoom();
      final row = _rowAfter(room.rows.last);
      room.store.push(row);
      room.bump();
      await _settle();
      expect(_shown(room.container), contains(row.id));

      // The relay's next window is asked after the row was fetched.
      await Future<void>.delayed(const Duration(milliseconds: 1100));
      await room.relayAgain();
      expect(_shown(room.container), isNot(contains(row.id)));

      room.bump();
      await _settle();
      expect(_shown(room.container), isNot(contains(row.id)));
    });

    test('every relay page moves the cut, not only the first', () async {
      final room = await _keptRoom();
      final row = _rowAfter(room.rows.last);
      await Future<void>.delayed(const Duration(milliseconds: 1100));
      final fetchedAt = relayAskTime();
      await Future<void>.delayed(const Duration(milliseconds: 1100));
      await room.relayAgain();

      room.store.push(row, fetchedAt: fetchedAt);
      room.bump();
      await _settle();
      expect(_shown(room.container), isNot(contains(row.id)));
    });

    test('a reconnect drops a reply it lacks instead of keeping it', () async {
      final room = await _keptRoom();
      final reply = fixtureRow(
        _channelId,
        7,
        room.rows.last.createdAt + 1,
        extraTags: [
          ['e', room.rows.last.id, '', 'reply'],
        ],
      );
      room.store.push(reply);
      room.bump();
      await _settle();
      expect(_shown(room.container), contains(reply.id));

      room.session.disconnect();
      await _settle();
      room.session.connect();
      await _settle();
      expect(_shown(room.container), isNot(contains(reply.id)));

      // Nor does it stay as reply evidence for the next draw.
      expect(room.notifier.cachedThreadReplyIds(room.rows.last.id), isEmpty);
      await room.notifier.fetchOlder();
      await _settle();
      expect(_shown(room.container), isNot(contains(reply.id)));
    });

    test('a live copy of the row survives the next relay page', () async {
      final room = await _keptRoom();
      final row = _rowAfter(room.rows.last);
      room.store.push(row);
      room.bump();
      await _settle();

      room.session.emit(row);
      await _settle();
      await room.relayAgain();
      expect(_shown(room.container), contains(row.id));
    });

    test('joins a room reconnecting, in order', () async {
      final room = await _keptRoom();
      room.session.disconnect();
      await _settle();
      room.session.headGate = Completer<void>();
      room.session.connect();
      await _settle();

      final row = _rowAfter(room.rows.last);
      room.store.push(row);
      room.bump();
      await _settle();
      expect(_shown(room.container).last, row.id);
    });

    test('a relay page from a middle window moves the cut', () async {
      final room = await _keptRoom(count: 200);
      await room.notifier.openAt(
        createdAt: room.rows[20].createdAt,
        eventId: room.rows[20].id,
      );
      await _settle();
      await Future<void>.delayed(const Duration(milliseconds: 1100));
      await room.notifier.jumpToLatest();
      await _settle();

      final row = _rowAfter(room.rows.last);
      room.store.push(row, fetchedAt: relayAskTime() - 1);
      room.bump();
      await _settle();
      expect(_shown(room.container), isNot(contains(row.id)));
    });

    test('waits for pages not loaded yet', () async {
      final room = await _keptRoom(count: 200);
      await room.notifier.openAt(
        createdAt: room.rows[20].createdAt,
        eventId: room.rows[20].id,
      );
      await _settle();
      final row = _rowAfter(room.rows.last);
      room.store.push(row);
      room.bump();
      await _settle();
      expect(_shown(room.container), isNot(contains(row.id)));
    });

    test('confirms no send', () async {
      final room = await _keptRoom();
      final row = _rowAfter(room.rows.last);
      room.notifier.addLocalMessage(row);
      room.store.push(row);
      room.bump();
      await _settle();
      expect(
        room.container.read(pendingLocalMessagesProvider(_channelId)).keys,
        [row.id],
      );
    });

    test('older pages still load under it', () async {
      final room = await _keptRoom(count: 200);
      final row = _rowAfter(room.rows.last);
      room.store.push(row);
      room.bump();
      await _settle();

      expect(await room.notifier.fetchOlder(), isTrue);
      await _settle();
      expect(_shown(room.container), hasLength(room.shownAtStart + 51));
      expect(_shown(room.container).last, row.id);
    });

    test('opening at the row needs no relay request', () async {
      final room = await _keptRoom();
      final row = _rowAfter(room.rows.last);
      room.store.push(row);
      room.bump();
      await _settle();
      final asked = room.session.queries.length;

      expect(await room.notifier.openAtRow(at(row), unread: true), isTrue);
      expect(room.session.queries, hasLength(asked));
    });

    test('reply counts: the store\'s newest, the higher count', () async {
      final room = await _keptRoom();
      final root = room.rows.last;
      room.session.overlays.add(
        _summary(root.id, replies: 5, descendants: 7, last: 100, who: 'a'),
      );
      await room.relayAgain();

      final at = relayAskTime() + 100;
      room.store.overlays.add(
        _summary(root.id, replies: 2, descendants: 2, last: at, who: 'b'),
      );
      room.bump();
      await _settle();
      final summary = room.notifier.threadSummaries[root.id]!;
      expect(summary.replyCount, 5);
      expect(summary.descendantCount, 7);
      expect(summary.lastReplyAt, at);
      expect(summary.participantPubkeys, ['b']);

      await room.relayAgain();
      expect(room.notifier.threadSummaries[root.id]!.lastReplyAt, 100);
    });

    test('an old reply count stays out', () async {
      final room = await _keptRoom();
      final root = room.rows.last;
      room.store.overlays.add(
        _summary(root.id, replies: 3, descendants: 3, last: 50, who: 'b'),
      );
      room.bump();
      await _settle();
      expect(room.notifier.threadSummaries[root.id], isNull);
    });
  });

  group('a certain window from the store', () {
    test('rules on nothing: it sets no cut', () async {
      final rows = [for (var n = 1; n <= 60; n++) _row(n)];
      final session = _Session(rows);
      final store = _Store(rows.toList());
      final container = _storeContainer(
        session,
        store,
        certain: () async => serveChannelWindow(
          _channelId,
          rows,
          NostrFilter(kinds: const [], limit: 50),
        ),
      );
      container.listen(channelMessagesProvider(_channelId), (_, _) {});
      await _settle();
      expect(_shown(container).last, rows.last.id);
      expect(session.queries, hasLength(1), reason: 'relay still out');

      final row = _rowAfter(rows.last);
      final older = fixtureRow(_channelId, 9, rows[55].createdAt);
      store.rows.addAll([row, older]);
      container.read(pushInboxRevisionProvider.notifier).bump();
      await _settle();
      expect(_shown(container).last, row.id);
      expect(_shown(container), isNot(contains(older.id)));
    });

    test('keeps rows the store added while it was read', () async {
      final rows = [for (var n = 1; n <= 60; n++) _row(n)];
      final session = _Session(rows);
      final store = _Store(rows.toList());
      final gate = Completer<void>();
      final container = _storeContainer(
        session,
        store,
        certain: () async {
          await gate.future;
          return serveChannelWindow(
            _channelId,
            rows,
            NostrFilter(kinds: const [], limit: 50),
          );
        },
      );
      container.listen(channelMessagesProvider(_channelId), (_, _) {});
      await _settle();
      final notifier = container.read(
        channelMessagesProvider(_channelId).notifier,
      );

      // A send over the saved rows: a store read no longer draws over them.
      notifier.addLocalMessage(_rowAfter(rows.last, 2));
      final row = _rowAfter(rows.last);
      store.push(row);
      container.read(pushInboxRevisionProvider.notifier).bump();
      await _settle();
      expect(_shown(container), contains(row.id));

      gate.complete();
      await _settle();
      expect(notifier.isShowingCachedHead, isFalse);
      expect(notifier.openOnRowsAtHand(at(row), unread: true), isTrue);
    });
  });

  group('a room nobody listened to', () {
    test('reads the store once as a listener comes back', () async {
      final room = await _keptRoom();
      final listening = room.listening;
      listening.close();
      await _settle();
      final row = _rowAfter(room.rows.last);
      room.store.push(row);
      room.bump();
      await _settle();
      expect(room.store.paints, 1);

      room.container.listen(channelMessagesProvider(_channelId), (_, _) {});
      await _settle();
      expect(room.store.paints, 2);
      expect(_shown(room.container).last, row.id);

      room.container.listen(channelMessagesProvider(_channelId), (_, _) {});
      await _settle();
      expect(room.store.paints, 2);
    });

    test('that went dirty reads it through its rebuild', () async {
      final room = await _keptRoom();
      room.listening.close();
      await _settle();
      final row = _rowAfter(room.rows.last);
      room.store.push(row);
      room.bump();
      room.session.disconnect();
      await _settle();
      room.session.headGate = Completer<void>();
      room.session.connect();
      await _settle();

      // The relay has yet to answer.
      room.container.listen(channelMessagesProvider(_channelId), (_, _) {});
      await _settle();
      expect(room.store.paints, 2);
      expect(_shown(room.container).last, row.id);
    });

    test('opening at a row waits for the read it is owed', () async {
      final room = await _keptRoom();
      room.listening.close();
      await _settle();
      final row = _rowAfter(room.rows.last);
      room.store.push(row);
      room.bump();
      await _settle();
      final asked = room.session.queries.length;
      final gate = room.store.gate = Completer<void>();

      room.container.listen(channelMessagesProvider(_channelId), (_, _) {});
      var opened = false;
      final opening = room.notifier
          .openAtRow(at(row), unread: true)
          .then((value) => opened = value);
      await _settle();
      expect(opened, isFalse);

      gate.complete();
      await opening;
      expect(opened, isTrue);
      expect(room.session.queries, hasLength(asked));
    });
  });

  test('a file the push service wrote shows in a room kept open', () async {
    sqfliteFfiInit();
    const relay = 'wss://relay.example';
    final account = 'a' * 64;
    const token = 'abababababababababababababababab';
    final dir = await Directory.systemTemp.createTemp('kept_room_drain');
    addTearDown(() => dir.delete(recursive: true));
    final store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => '${dir.path}/store',
    );
    final lease = store.lease(relay, account)!;
    addTearDown(() => store.forget(relay, account));
    store.forgetRoomsExcept(store.capture(lease), lease, {_channelId});
    await store.flushed();

    final rows = [for (var n = 1; n <= 60; n++) _row(n)];
    final session = _Session(rows)..headGate.complete();
    final container = _container(
      session,
      paint: (filter) async {
        final head = await store.paintableRoomHead(
          lease,
          room: _channelId,
          kinds: filter.kinds,
          limit: filter.limit,
        );
        return head?.map(NostrEvent.fromJson).toList();
      },
    );
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    await _settle();
    expect(_shown(container).last, rows.last.id);

    final row = _rowAfter(rows.last);
    final folder = await Directory(
      '${dir.path}/inbox/$token',
    ).create(recursive: true);
    await File('${folder.path}/1.json').writeAsString(
      jsonEncode({
        'v': 1,
        'relayUrl': relay,
        'account': account,
        'fetchedAt': relayAskTime(),
        'events': [
          {...row.toJson(), 'sig': '0' * 128},
        ],
      }),
    );
    await drainPushInbox(
      store: store,
      scope: (lease: lease, token: token, relayUrl: relay),
      root: '${dir.path}/inbox',
      current: () => true,
      folds: const [],
      cacheProfile: (_) {},
      onWritten: container.read(pushInboxRevisionProvider.notifier).bump,
    );
    await expectLater(() async {
      for (var i = 0; i < 200; i++) {
        if (_shown(container).contains(row.id)) return true;
        await Future<void>.delayed(const Duration(milliseconds: 5));
      }
      return false;
    }(), completion(isTrue));
    expect(_shown(container).last, row.id);
  });

  testWidgets('a paused room reads the store as it is shown again', (
    tester,
  ) async {
    final rows = [for (var n = 1; n <= 60; n++) _row(n)];
    final session = _Session(rows)..headGate.complete();
    final store = _Store(rows.toList());
    final container = _storeContainer(session, store);
    final shown = ValueNotifier(true);
    addTearDown(shown.dispose);
    Future<void> pump() async {
      for (var i = 0; i < 8; i++) {
        await tester.pump();
      }
    }

    await tester.pumpWidget(
      UncontrolledProviderScope(
        container: container,
        child: ValueListenableBuilder<bool>(
          valueListenable: shown,
          builder: (_, value, child) =>
              TickerMode(enabled: value, child: child!),
          child: Consumer(
            builder: (_, ref, _) {
              ref.watch(channelMessagesProvider(_channelId));
              return const SizedBox();
            },
          ),
        ),
      ),
    );
    await pump();
    expect(_shown(container).last, rows.last.id);

    shown.value = false;
    await pump();
    final row = _rowAfter(rows.last);
    store.push(row);
    container.read(pushInboxRevisionProvider.notifier).bump();
    await pump();
    expect(store.paints, 1, reason: 'a paused room does not read');

    shown.value = true;
    await pump();
    expect(store.paints, 2);
    expect(_shown(container).last, row.id);
  });

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

  test("an account switch drops the previous account's rows at once", () async {
    final rows = [for (var n = 1; n <= 60; n++) _row(n)];
    // The relay's rows for the first account only.
    final relayRows = [...rows];
    final stored = rows.sublist(0, 55);
    final session = _Session(relayRows);
    var current = 'me';
    final account = NotifierProvider<_Account, String>(_Account.new);
    final container = ProviderContainer(
      overrides: [
        relaySessionProvider.overrideWith(() => session),
        myPubkeyProvider.overrideWith((ref) => ref.watch(account)),
        channelHeadScopeProvider.overrideWith(
          (ref) =>
              ChannelHeadScope(relayUrl: 'wss://r', pubkey: ref.watch(account)),
        ),
        channelHeadCacheProvider.overrideWithValue(_NoHeadCache()),
        // Only the first account has saved rows.
        paintableRoomHeadProvider.overrideWithValue(
          (_, _, filter) async => current == 'me'
              ? serveChannelWindow(_channelId, stored, filter)
              : null,
        ),
        relayInfoProvider.overrideWith(
          (ref) async => RelayInfo(supportedExtensions: {newerWindowExtension}),
        ),
        storedRoomHeadProvider.overrideWithValue((_, _, _) async => null),
      ],
    );
    addTearDown(container.dispose);
    final seenAfterSwitch = <String>{};
    var switched = false;
    container.listen(channelMessagesProvider(_channelId), (_, next) {
      if (switched) {
        seenAfterSwitch.addAll([for (final e in next.value ?? const []) e.id]);
      }
    });
    session.headGate.complete();
    await _settle();
    expect(_shown(container).last, rows.last.id);

    // A row shown only from the store goes with the account too.
    stored.add(fixtureRow(_channelId, 7, relayAskTime() + 100));
    container.read(pushInboxRevisionProvider.notifier).bump();
    await _settle();
    expect(_shown(container).last, stored.last.id);

    switched = true;
    current = 'other';
    relayRows.clear();
    container.read(account.notifier).state = 'other';
    expect(_shown(container), isEmpty);
    await _settle();
    expect(seenAfterSwitch, isEmpty);
    expect(_shown(container), isEmpty);
  });
}

class _Account extends Notifier<String> {
  @override
  String build() => 'me';

  @override
  set state(String value) => super.state = value;
}
