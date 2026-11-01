import 'dart:async';
import 'dart:convert';

import 'package:fake_async/fake_async.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:shared_preferences/shared_preferences.dart';
import 'package:kuzz/shared/read_state/read_state_format.dart';
import 'package:kuzz/shared/read_state/read_state_manager.dart';
import 'package:kuzz/shared/relay/relay.dart';

void main() {
  test('dispose flushes a pending publish after marking disposed', () async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    final keychain = nostr.Keys.generate();
    final nsec = keychain.nsec;
    final crypto = ReadStateCrypto.tryCreate(
      nsec: nsec,
      pubkey: keychain.public,
    );
    final relay = _FakeSignedEventRelay();
    final manager = ReadStateManager(
      pubkey: keychain.public,
      prefs: prefs,
      crypto: crypto!,
      relaySession: null,
      signedEventRelay: relay,
      remoteEnabled: true,
      onChanged: () {},
    );

    manager.markContextRead('channel-1', 42);
    manager.dispose();

    final submitted = await relay.submitted.future.timeout(
      const Duration(seconds: 1),
    );
    expect(submitted.kind, EventKind.readState);
    expect(
      submitted.tags.any(
        (tag) => tag.length == 2 && tag[0] == 't' && tag[1] == 'read-state',
      ),
      isTrue,
    );
  });

  test('disables remote sync after relay rejects read-state kind', () async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    final keychain = nostr.Keys.generate();
    final nsec = keychain.nsec;
    final crypto = ReadStateCrypto.tryCreate(
      nsec: nsec,
      pubkey: keychain.public,
    );
    final relay = _UnsupportedKindSignedEventRelay();
    final manager = ReadStateManager(
      pubkey: keychain.public,
      prefs: prefs,
      crypto: crypto!,
      relaySession: null,
      signedEventRelay: relay,
      remoteEnabled: true,
      onChanged: () {},
    );

    manager.markContextRead('channel-1', 42);
    await manager.flush();

    manager.markContextRead('channel-2', 43);
    await manager.flush();

    expect(relay.submitCount, 1);
    expect(manager.getEffectiveTimestamp('channel-2'), 43);
  });

  test(
    'disables remote sync after token permanently lacks write scope',
    () async {
      SharedPreferences.setMockInitialValues({});
      final prefs = await SharedPreferences.getInstance();
      final keychain = nostr.Keys.generate();
      final nsec = keychain.nsec;
      final crypto = ReadStateCrypto.tryCreate(
        nsec: nsec,
        pubkey: keychain.public,
      );
      final relay = _MissingScopeSignedEventRelay();
      final manager = ReadStateManager(
        pubkey: keychain.public,
        prefs: prefs,
        crypto: crypto!,
        relaySession: null,
        signedEventRelay: relay,
        remoteEnabled: true,
        onChanged: () {},
      );

      manager.markContextRead('channel-1', 42);
      await manager.flush();

      manager.markContextRead('channel-2', 43);
      await manager.flush();

      expect(relay.submitCount, 1);
      expect(manager.getEffectiveTimestamp('channel-2'), 43);
    },
  );

  test('disables remote sync after an oversized local blob', () async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    final keychain = nostr.Keys.generate();
    final crypto = ReadStateCrypto.tryCreate(
      nsec: keychain.nsec,
      pubkey: keychain.public,
    )!;
    final relay = _FakeSignedEventRelay();
    final manager = ReadStateManager(
      pubkey: keychain.public,
      prefs: prefs,
      crypto: crypto,
      relaySession: null,
      signedEventRelay: relay,
      remoteEnabled: true,
      onChanged: () {},
    );

    for (var index = 0; index < 1400; index++) {
      manager.markContextRead(
        'channel-${index.toString().padLeft(4, '0')}-${'x' * 48}',
        index + 1,
      );
    }
    await manager.flush();

    manager.markContextRead('channel-new', 2000);
    await manager.flush();

    expect(relay.submitCount, 0);
    expect(manager.getEffectiveTimestamp('channel-0000-${'x' * 48}'), 1);
    expect(manager.getEffectiveTimestamp('channel-new'), 2000);
  });

  test('remote read-state rollback is ignored', () async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    final keychain = nostr.Keys.generate();
    final crypto = ReadStateCrypto.tryCreate(
      nsec: keychain.nsec,
      pubkey: keychain.public,
    );
    final relay = _FakeRelaySession();
    final manager = ReadStateManager(
      pubkey: keychain.public,
      prefs: prefs,
      crypto: crypto!,
      relaySession: relay,
      signedEventRelay: _FakeSignedEventRelay(),
      remoteEnabled: true,
      onChanged: () {},
    );

    relay.historyEvents = [
      _readStateEvent(
        pubkey: keychain.public,
        crypto: crypto,
        clientId: 'remote-client',
        slotId: 'remote-slot',
        contexts: {'channel-1': 100},
        createdAt: 100,
      ),
      _readStateEvent(
        pubkey: keychain.public,
        crypto: crypto,
        clientId: 'remote-client',
        slotId: 'remote-slot',
        contexts: {'channel-1': 50},
        createdAt: 110,
      ),
    ];

    await manager.initialize();

    expect(manager.getEffectiveTimestamp('channel-1'), 100);
  });

  group('live-only listener on the account bundle', () {
    late SharedPreferences prefs;
    late nostr.Keys keys;
    late ReadStateCrypto crypto;
    late _FakeRelaySession relay;
    late ReadStateManager manager;

    setUp(() async {
      SharedPreferences.setMockInitialValues({});
      prefs = await SharedPreferences.getInstance();
      keys = nostr.Keys.generate();
      crypto = ReadStateCrypto.tryCreate(nsec: keys.nsec, pubkey: keys.public)!;
      relay = _FakeRelaySession();
      manager = ReadStateManager(
        pubkey: keys.public,
        prefs: prefs,
        crypto: crypto,
        relaySession: relay,
        signedEventRelay: _FakeSignedEventRelay(),
        remoteEnabled: true,
        onChanged: () {},
      );
    });
    tearDown(() => manager.dispose(flushPending: false));

    NostrEvent otherDevice(Map<String, int> contexts) => _readStateEvent(
      pubkey: keys.public,
      crypto: crypto,
      clientId: 'other-device',
      slotId: 'other-slot',
      contexts: contexts,
      createdAt: 100,
    );

    test('listens live-only first; a reconnect only fetches', () async {
      await manager.initialize();
      expect(relay.calls, ['live', 'fetch']);
      expect(relay.liveFilters.single.limit, 0);
      expect(relay.liveFilters.single.since, isNull);
      expect(relay.fetchFilters.single.limit, readStateFetchLimit);

      relay.historyEvents = [
        otherDevice({'channel-1': 300}),
      ];
      await manager.reinitializeRemote();
      expect(relay.calls, ['live', 'fetch', 'fetch']);
      expect(manager.getEffectiveTimestamp('channel-1'), 300);
    });

    test('a failed fetch with the socket up retries and merges', () {
      fakeAsync((clock) {
        relay
          ..fetchFailures = 2
          ..historyEvents = [
            otherDevice({'channel-1': 500}),
          ];
        manager.initialize();
        clock.flushMicrotasks();
        expect(manager.getEffectiveTimestamp('channel-1'), isNull);
        clock.elapse(const Duration(seconds: 1));
        expect(manager.getEffectiveTimestamp('channel-1'), isNull);
        clock.elapse(const Duration(seconds: 2));
        expect(manager.getEffectiveTimestamp('channel-1'), 500);
        expect(relay.fetchFilters, hasLength(3));
        clock.elapse(const Duration(minutes: 5));
        expect(
          relay.fetchFilters,
          hasLength(3),
          reason: 'no retry after success',
        );
      });
    });

    test('a terminal CLOSED re-attaches one listener with backoff', () {
      fakeAsync((clock) {
        manager.initialize();
        clock.flushMicrotasks();
        relay.closeLive('error: too many subscriptions');
        clock.flushMicrotasks();
        expect(relay.liveFilters, hasLength(1), reason: 'no immediate loop');
        clock.elapse(const Duration(milliseconds: 999));
        expect(relay.liveFilters, hasLength(1));
        clock.elapse(const Duration(milliseconds: 1));
        expect(relay.liveFilters, hasLength(2));
        expect(
          relay.fetchFilters,
          hasLength(2),
          reason: 'reads what it missed',
        );
        clock.elapse(const Duration(minutes: 5));
        expect(relay.liveFilters, hasLength(2));
      });
    });

    test('a catch-up gap refetches', () async {
      await manager.initialize();
      relay.historyEvents = [
        otherDevice({'channel-2': 700}),
      ];
      relay.gap();
      await Future<void>.delayed(Duration.zero);
      expect(relay.fetchFilters, hasLength(2));
      expect(manager.getEffectiveTimestamp('channel-2'), 700);
    });
  });
}

class _SubmittedEvent {
  final int kind;
  final List<List<String>> tags;

  const _SubmittedEvent({required this.kind, required this.tags});
}

/// Build a stub NostrEvent for tests that just need a "ack" return value.
NostrEvent _stubAckEvent() => const NostrEvent(
  id: 'stub',
  pubkey: '',
  createdAt: 0,
  kind: 0,
  tags: [],
  content: '',
  sig: '',
);

class _FakeSignedEventRelay implements SignedEventRelay {
  final Completer<_SubmittedEvent> submitted = Completer<_SubmittedEvent>();
  int submitCount = 0;

  @override
  String? get pubkey => null;

  @override
  Future<NostrEvent> submit({
    required int kind,
    required String content,
    required List<List<String>> tags,
    int? createdAt,
    void Function(NostrEvent event)? onSigned,
  }) async {
    submitCount++;
    submitted.complete(_SubmittedEvent(kind: kind, tags: tags));
    return _stubAckEvent();
  }
}

class _UnsupportedKindSignedEventRelay implements SignedEventRelay {
  int submitCount = 0;

  @override
  String? get pubkey => null;

  @override
  Future<NostrEvent> submit({
    required int kind,
    required String content,
    required List<List<String>> tags,
    int? createdAt,
    void Function(NostrEvent event)? onSigned,
  }) async {
    submitCount++;
    throw Exception('restricted: unknown event kind');
  }
}

class _MissingScopeSignedEventRelay implements SignedEventRelay {
  int submitCount = 0;

  @override
  String? get pubkey => null;

  @override
  Future<NostrEvent> submit({
    required int kind,
    required String content,
    required List<List<String>> tags,
    int? createdAt,
    void Function(NostrEvent event)? onSigned,
  }) async {
    submitCount++;
    throw Exception('missing users:write');
  }
}

NostrEvent _readStateEvent({
  required String pubkey,
  required ReadStateCrypto crypto,
  required String clientId,
  required String slotId,
  required Map<String, int> contexts,
  required int createdAt,
}) {
  final blob = ReadStateBlob(clientId: clientId, contexts: contexts);
  return NostrEvent(
    id: 'event-$clientId-$createdAt',
    pubkey: pubkey,
    createdAt: createdAt,
    kind: EventKind.readState,
    tags: [
      ['d', '$readStateDTagPrefix$slotId'],
      ['t', 'read-state'],
    ],
    content: crypto.encrypt(jsonEncode(blob.toJson())),
    sig: 'sig',
  );
}

class _FakeRelaySession extends RelaySessionNotifier {
  List<NostrEvent> historyEvents = [];
  int fetchFailures = 0;
  final calls = <String>[];
  final fetchFilters = <NostrFilter>[];
  final liveFilters = <NostrFilter>[];
  void Function(String message)? _onClosed;
  void Function()? _onGap;

  void closeLive(String message) => _onClosed?.call(message);
  void gap() => _onGap?.call();

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    calls.add('fetch');
    fetchFilters.add(filter);
    if (fetchFailures > 0) {
      fetchFailures--;
      throw TimeoutException('read state timed out');
    }
    return historyEvents;
  }

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
    void Function()? onGap,
  }) async {
    calls.add('live');
    liveFilters.add(filter);
    _onClosed = onClosed;
    _onGap = onGap;
    return () {};
  }
}
