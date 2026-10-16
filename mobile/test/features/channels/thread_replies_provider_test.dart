import 'dart:async';

import 'package:kuzz/features/channels/pending_local_messages_provider.dart';
import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

class _FakeRelaySession extends RelaySessionNotifier {
  int queryCount = 0;
  bool honorDepthLimit = false;
  final filtersSeen = <NostrFilter>[];
  List<NostrEvent> replies = const [];
  Completer<List<NostrEvent>>? nextQueryGate;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  void setStatus(SessionStatus status) {
    state = SessionState(status: status);
  }

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    // A relay without thread windows: the thread query falls back to its
    // complete scan, which these tests exercise.
    if (filters.any((filter) => filter.extensions['thread_window'] == true)) {
      return const [];
    }
    queryCount++;
    filtersSeen.addAll(filters);
    final gate = nextQueryGate;
    if (gate != null) {
      nextQueryGate = null;
      return gate.future;
    }
    if (honorDepthLimit) {
      final depth = filters.single.extensions['depth_limit'] as int;
      return replies.where((event) => event.createdAt <= depth).toList();
    }
    return replies;
  }
}

NostrEvent _reply(String id, int createdAt) => NostrEvent(
  id: id,
  pubkey: 'bob',
  createdAt: createdAt,
  kind: EventKind.streamMessage,
  tags: const [
    ['h', 'chan'],
    ['e', 'root', '', 'reply'],
  ],
  content: 'reply $id',
  sig: '',
);

void main() {
  const args = ThreadRepliesArgs(channelId: 'chan', rootId: 'root');

  test('complete scan includes a legal 80-deep reply chain', () async {
    final session = _FakeRelaySession()
      ..honorDepthLimit = true
      ..replies = [
        for (var depth = 1; depth <= 80; depth++)
          NostrEvent(
            id: 'reply-$depth',
            pubkey: 'bob',
            createdAt: depth,
            kind: EventKind.streamMessage,
            tags: [
              ['h', 'chan'],
              ['e', 'root', '', 'root'],
              ['e', depth == 1 ? 'root' : 'reply-${depth - 1}', '', 'reply'],
            ],
            content: '',
            sig: '',
          ),
      ];
    final container = ProviderContainer(
      overrides: [relaySessionProvider.overrideWith(() => session)],
    );
    addTearDown(container.dispose);
    container.listen(threadRepliesProvider(args), (_, _) {});
    expect(
      await container.read(threadRepliesProvider(args).future),
      hasLength(80),
    );
  });

  test(
    'origin cursor includes epoch-zero events in an exhaustive scan',
    () async {
      final session = _FakeRelaySession()..replies = [_reply('epoch', 0)];
      final container = ProviderContainer(
        overrides: [relaySessionProvider.overrideWith(() => session)],
      );
      addTearDown(container.dispose);
      container.listen(threadRepliesProvider(args), (_, _) {});
      expect(
        (await container.read(
          threadRepliesProvider(args).future,
        )).single.createdAt,
        0,
      );
      expect(session.filtersSeen.single.extensions['thread_cursor'], -1);
      expect(
        session.filtersSeen.single.extensions['thread_cursor_id'],
        '0' * 64,
      );
    },
  );

  test(
    'disposes empty local overlays after their last listener leaves',
    () async {
      final container = ProviderContainer();
      addTearDown(container.dispose);
      final provider = threadLocalRepliesProvider(args);
      final subscription = container.listen(provider, (_, _) {});
      subscription.close();
      await container.pump();
      expect(container.exists(provider), isFalse);
    },
  );

  test('retains local replies without listeners until confirmation', () async {
    final container = ProviderContainer();
    addTearDown(container.dispose);
    final provider = threadLocalRepliesProvider(args);
    final notifier = container.read(provider.notifier);
    notifier.add(_reply('pending', 1000));
    await container.pump();
    expect(container.read(provider).single.id, 'pending');
    notifier.confirm({'pending'});
    await container.pump();
    expect(container.exists(provider), isFalse);
  });

  (ProviderContainer, _FakeRelaySession, ProviderSubscription<Object?>)
  makeHarness(List<NostrEvent> initialReplies) {
    final fakeSession = _FakeRelaySession()..replies = initialReplies;
    final container = ProviderContainer(
      overrides: [relaySessionProvider.overrideWith(() => fakeSession)],
    );
    // An auto-disposed provider needs a listener to stay alive, mirroring an
    // open thread page. Creating it starts the first load, so the fake's
    // replies must be in place first.
    final subscription = container.listen(
      threadRepliesProvider(args),
      (_, _) {},
    );
    return (container, fakeSession, subscription);
  }

  test(
    'a closed thread keeps its replies and reopens without a refetch',
    () async {
      final (container, fakeSession, subscription) = makeHarness([
        _reply('r1', 1000),
      ]);
      addTearDown(container.dispose);

      await container.read(threadRepliesProvider(args).future);
      final queriesAfterFirstLoad = fakeSession.queryCount;

      // Close the thread page, then reopen it.
      subscription.close();
      await container.pump();
      final reopened = container.listen(threadRepliesProvider(args), (_, _) {});
      addTearDown(reopened.close);

      expect(
        container.read(threadRepliesProvider(args)).value?.map((e) => e.id),
        ['r1'],
      );
      await container.pump();
      expect(fakeSession.queryCount, queriesAfterFirstLoad);
    },
  );

  test(
    'a reply while the thread is closed refreshes only when it reopens',
    () async {
      final (container, fakeSession, subscription) = makeHarness([
        _reply('r1', 1000),
      ]);
      addTearDown(container.dispose);

      await container.read(threadRepliesProvider(args).future);
      final queriesAfterFirstLoad = fakeSession.queryCount;
      subscription.close();
      await container.pump();

      // A live reply marks the closed thread stale: no fetch yet.
      fakeSession.replies = [_reply('r1', 1000), _reply('r2', 2000)];
      container.read(threadRepliesStaleProvider(args).notifier).markStale();
      await container.pump();
      expect(fakeSession.queryCount, queriesAfterFirstLoad);

      // Reopening shows the kept replies while one refresh runs.
      final reopened = container.listen(threadRepliesProvider(args), (_, _) {});
      addTearDown(reopened.close);
      expect(
        container.read(threadRepliesProvider(args)).value?.map((e) => e.id),
        ['r1'],
      );
      // The held refresh starts right after the reopen.
      await container.pump();
      final refreshed = await container.read(
        threadRepliesProvider(args).future,
      );
      expect(refreshed.map((event) => event.id), ['r1', 'r2']);
      expect(fakeSession.queryCount, queriesAfterFirstLoad + 1);
    },
  );

  test('thread replies keep desktop same-second id order', () {
    expect(
      mergeThreadEvents(
        [_reply('z', 1000)],
        [_reply('a', 1000), _reply('m', 1000)],
      ).map((event) => event.id),
      ['a', 'm', 'z'],
    );
  });

  test('does not refetch on the disconnect edge', () async {
    final (container, fakeSession, _) = makeHarness([_reply('r1', 1000)]);
    addTearDown(container.dispose);

    await container.read(threadRepliesProvider(args).future);
    final queriesAfterFirstLoad = fakeSession.queryCount;

    fakeSession.setStatus(SessionStatus.disconnected);
    await container.pump();

    expect(fakeSession.queryCount, queriesAfterFirstLoad);
  });

  test('refetches exactly once per reconnect edge', () async {
    final (container, fakeSession, _) = makeHarness([_reply('r1', 1000)]);
    addTearDown(container.dispose);

    final first = await container.read(threadRepliesProvider(args).future);
    expect(first.map((event) => event.id), ['r1']);
    final queriesAfterFirstLoad = fakeSession.queryCount;

    // A reply lands while the connection is down.
    fakeSession.replies = [_reply('r1', 1000), _reply('r2', 2000)];
    fakeSession.setStatus(SessionStatus.disconnected);
    await container.pump();
    fakeSession.setStatus(SessionStatus.connected);
    await container.pump();

    final second = await container.read(threadRepliesProvider(args).future);
    expect(second.map((event) => event.id), ['r1', 'r2']);
    expect(fakeSession.queryCount, queriesAfterFirstLoad + 1);
  });

  test(
    'does not refetch on session emissions that keep the same status',
    () async {
      final (container, fakeSession, _) = makeHarness([_reply('r1', 1000)]);
      addTearDown(container.dispose);

      await container.read(threadRepliesProvider(args).future);
      final queriesAfterFirstLoad = fakeSession.queryCount;

      // Same connected status, new state object (e.g. reconnectAttempt bump).
      fakeSession.setStatus(SessionStatus.connected);
      await container.pump();

      expect(fakeSession.queryCount, queriesAfterFirstLoad);
    },
  );

  test('keeps previous replies available while a refresh is pending', () async {
    final (container, fakeSession, _) = makeHarness([_reply('r1', 1000)]);
    addTearDown(container.dispose);

    await container.read(threadRepliesProvider(args).future);

    // Hold the reconnect refresh open and verify the old data still reads.
    final gate = Completer<List<NostrEvent>>();
    fakeSession.nextQueryGate = gate;
    fakeSession.setStatus(SessionStatus.disconnected);
    await container.pump();
    fakeSession.setStatus(SessionStatus.connected);
    await container.pump();

    final pending = container.read(threadRepliesProvider(args));
    expect(pending.isLoading, isTrue);
    expect(pending.value?.map((event) => event.id), ['r1']);

    gate.complete([_reply('r1', 1000), _reply('r2', 2000)]);
    final refreshed = await container.read(threadRepliesProvider(args).future);
    expect(refreshed.map((event) => event.id), ['r1', 'r2']);
  });

  test(
    'confirmation survives disposing the combined provider before its microtask',
    () async {
      final reply = _reply('r1', 1000);
      final query = Completer<List<NostrEvent>>();
      final fakeSession = _FakeRelaySession()..nextQueryGate = query;
      final container = ProviderContainer(
        overrides: [relaySessionProvider.overrideWith(() => fakeSession)],
      );
      addTearDown(container.dispose);
      container.read(threadLocalRepliesProvider(args).notifier).add(reply);
      container
          .read(pendingLocalMessagesProvider(args.channelId).notifier)
          .add(reply);

      late ProviderSubscription<AsyncValue<List<NostrEvent>>> subscription;
      subscription = container.listen(threadRepliesWithLocalProvider(args), (
        _,
        next,
      ) {
        if (next.value?.any((event) => event.id == reply.id) ?? false) {
          subscription.close();
        }
      });
      query.complete([reply]);
      await container.pump();
      await Future<void>.delayed(Duration.zero);

      expect(container.read(threadLocalRepliesProvider(args)), isEmpty);
      expect(
        container.read(pendingLocalMessagesProvider(args.channelId)),
        isEmpty,
      );
    },
  );

  test(
    'mounted thread renders a reply missed while disconnected after reconnect',
    () async {
      final fakeSession = _FakeRelaySession()..replies = [_reply('r1', 1000)];
      final container = ProviderContainer(
        overrides: [relaySessionProvider.overrideWith(() => fakeSession)],
      );
      addTearDown(container.dispose);
      final mountedThread = container.listen(
        threadRepliesWithLocalProvider(args),
        (_, _) {},
      );
      addTearDown(mountedThread.close);

      await container.read(threadRepliesProvider(args).future);
      expect(mountedThread.read().value?.map((event) => event.id), ['r1']);

      fakeSession.setStatus(SessionStatus.disconnected);
      await container.pump();
      fakeSession.replies = [_reply('r1', 1000), _reply('r2', 2000)];
      fakeSession.setStatus(SessionStatus.connected);
      await container.pump();
      await container.read(threadRepliesProvider(args).future);

      expect(mountedThread.read().value?.map((event) => event.id), [
        'r1',
        'r2',
      ]);
    },
  );

  test(
    'local replies preserve cached authoritative replies after refresh failure',
    () async {
      final cached = _reply('cached', 1000);
      final local = _reply('local', 1001);
      final session = _FakeRelaySession()..replies = [cached];
      final container = ProviderContainer(
        retry: (_, _) => null,
        overrides: [relaySessionProvider.overrideWith(() => session)],
      );
      addTearDown(container.dispose);
      final combined = container.listen(
        threadRepliesWithLocalProvider(args),
        (_, _) {},
      );
      await container.read(threadRepliesProvider(args).future);
      container.read(threadLocalRepliesProvider(args).notifier).add(local);
      await container.pump();
      final refresh = Completer<List<NostrEvent>>();
      session.nextQueryGate = refresh;
      container.invalidate(threadRepliesProvider(args));
      await container.pump();
      expect(combined.read().value?.map((event) => event.id), [
        'cached',
        'local',
      ]);
      refresh.completeError(Exception('Refresh failed'));
      await container.pump();
      expect(container.read(threadRepliesProvider(args)).hasError, isTrue);
      expect(combined.read().value?.map((event) => event.id), [
        'cached',
        'local',
      ]);
      expect(container.read(threadLocalRepliesProvider(args)), [local]);
      session.replies = [cached, local];
      container.invalidate(threadRepliesProvider(args));
      await container.read(threadRepliesProvider(args).future);
      await container.pump();
      expect(combined.read().value?.map((event) => event.id), [
        'cached',
        'local',
      ]);
      await Future<void>.delayed(Duration.zero);
      expect(container.read(threadLocalRepliesProvider(args)), isEmpty);
    },
  );

  test('a thread released after retention reloads fresh', () async {
    final (container, fakeSession, subscription) = makeHarness([
      _reply('r1', 1000),
    ]);
    addTearDown(container.dispose);

    await container.read(threadRepliesProvider(args).future);
    final queriesAfterFirstLoad = fakeSession.queryCount;

    // Closing keeps the query; disposing it (retention over) drops it…
    subscription.close();
    await container.pump();
    expect(container.exists(threadRepliesProvider(args)), isTrue);
    container.invalidate(threadRepliesProvider(args));
    await container.pump();

    // …so the next open loads fresh instead of serving a stale cache.
    fakeSession.replies = [_reply('r1', 1000), _reply('r2', 2000)];
    container.listen(threadRepliesProvider(args), (_, _) {});
    final reopened = await container.read(threadRepliesProvider(args).future);
    expect(reopened.map((event) => event.id), ['r1', 'r2']);
    expect(fakeSession.queryCount, queriesAfterFirstLoad + 1);
  });

  test(
    'closed threads past the cap release the longest-closed first',
    () async {
      final session = _FakeRelaySession()..replies = [_reply('r1', 1000)];
      final container = ProviderContainer(
        overrides: [
          relaySessionProvider.overrideWith(() => session),
          closedThreadRetentionProvider.overrideWithValue(
            ClosedThreadRetention(maxThreads: 2),
          ),
        ],
      );
      addTearDown(container.dispose);
      final threads = [
        for (var i = 0; i < 3; i++)
          ThreadRepliesArgs(channelId: 'channel', rootId: 'root-$i'),
      ];
      for (final thread in threads) {
        final subscription = container.listen(
          threadRepliesProvider(thread),
          (_, _) {},
        );
        await container.read(threadRepliesProvider(thread).future);
        subscription.close();
        await container.pump();
      }
      await container.pump();

      expect(container.exists(threadRepliesProvider(threads[0])), isFalse);
      expect(container.exists(threadRepliesProvider(threads[1])), isTrue);
      expect(container.exists(threadRepliesProvider(threads[2])), isTrue);
      expect(container.read(closedThreadRetentionProvider).length, 2);

      // Reopening a kept thread takes it off the release list.
      final reopened = container.listen(
        threadRepliesProvider(threads[1]),
        (_, _) {},
      );
      addTearDown(reopened.close);
      expect(container.read(closedThreadRetentionProvider).length, 1);
    },
  );
}
