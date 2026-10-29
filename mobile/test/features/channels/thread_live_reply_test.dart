import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel_messages_provider.dart';
import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:kuzz/features/channels/thread_window.dart';
import 'package:kuzz/features/channels/window_anchor.dart';
import 'package:kuzz/shared/relay/relay.dart';

// A live reply in an open thread: folded into the thread's window when it
// shows the newest page (no relay request), refetched otherwise
// (docs/message-sync.md#open-room-and-thread-requests).

const _room = '22222222-2222-4222-8222-222222222222';
const _args = ThreadRepliesArgs(channelId: _room, rootId: 'root');

NostrEvent _event(String id, int createdAt, {String? parent}) => NostrEvent(
  id: id,
  pubkey: 'alice',
  createdAt: createdAt,
  kind: EventKind.streamMessageV2,
  tags: [
    ['h', _room],
    if (parent != null) ...[
      ['e', 'root', '', 'root'],
      ['e', parent, '', 'reply'],
    ],
  ],
  content: id,
  sig: 'sig',
);

NostrEvent _reply(String id, int createdAt) =>
    _event(id, createdAt, parent: 'root');

NostrEvent _threadBounds(
  String rootId, {
  bool hasMore = false,
  NostrEvent? next,
  String direction = 'older',
}) => NostrEvent(
  id: 'bounds-$rootId-$direction',
  pubkey: 'relay',
  createdAt: 0,
  kind: threadWindowBoundsKind,
  tags: [
    ['h', _room],
    ['e', rootId],
  ],
  content: jsonEncode({
    'version': 1,
    'direction': direction,
    'has_more': hasMore,
    'next_cursor': next == null
        ? null
        : {'created_at': next.createdAt, 'id': next.id},
  }),
  sig: '',
);

/// Serves the room's newest window, thread windows and complete scans, and
/// counts every request by type.
class _Relay extends RelaySessionNotifier {
  final requests = <String>[];
  final _listeners = <void Function(NostrEvent)>[];
  List<NostrEvent> threadReplies = [_reply('r1', 20)];
  bool failing = false;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    if (failing) throw Exception('relay unavailable');
    final filter = filters.single;
    final extensions = filter.extensions;
    if (extensions['top_level'] == true) {
      requests.add('room window');
      return [
        _event('root', 10),
        NostrEvent(
          id: 'room-bounds',
          pubkey: 'relay',
          createdAt: 0,
          kind: EventKind.channelWindowBounds,
          tags: [
            ['d', '$_room:head'],
          ],
          content: jsonEncode({'has_more': false, 'next_cursor': null}),
          sig: 'sig',
        ),
      ];
    }
    if (extensions['thread_window'] == true) {
      final rootId = filter.tags['#e']!.single;
      if (extensions['after_id'] != null) {
        requests.add('thread newer');
        // Opened at r1: more replies after it remain on the relay.
        return [
          _reply('r1b', 21),
          _threadBounds(
            rootId,
            hasMore: true,
            next: _reply('r1b', 21),
            direction: 'newer',
          ),
        ];
      }
      requests.add(
        extensions['before_id'] != null ? 'thread older' : 'thread window',
      );
      return [if (rootId == 'root') ...threadReplies, _threadBounds(rootId)];
    }
    if (extensions['depth_limit'] != null) {
      requests.add('thread scan');
      return threadReplies;
    }
    requests.add('other');
    return const [];
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

  void dropAndReconnect() {
    state = const SessionState(status: SessionStatus.disconnected);
    state = const SessionState(status: SessionStatus.connected);
  }

  void emit(NostrEvent event) {
    for (final listener in List.of(_listeners)) {
      listener(event);
    }
  }
}

Future<void> _settle() async {
  for (var i = 0; i < 6; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

/// The room and its thread open, both loaded. [retry] off stands for a
/// failure the app does not retry (a deadline) or retries given up.
Future<(ProviderContainer, _Relay)> _openThread({bool retry = true}) async {
  final relay = _Relay();
  final container = ProviderContainer(
    retry: retry ? null : (_, _) => null,
    overrides: [
      relaySessionProvider.overrideWith(() => relay),
      relayInfoProvider.overrideWith(
        (ref) async =>
            const RelayInfo(supportedExtensions: {newerWindowExtension}),
      ),
    ],
  );
  addTearDown(container.dispose);
  container.listen(channelMessagesProvider(_room), (_, _) {});
  await _settle();
  container.listen(threadRepliesWithLocalProvider(_args), (_, _) {});
  await container.read(threadRepliesProvider(_args).future);
  await _settle();
  return (container, relay);
}

List<String> _shown(ProviderContainer container) => [
  for (final event
      in container.read(threadRepliesWithLocalProvider(_args)).value ??
          const <NostrEvent>[])
    event.id,
];

/// How often the thread was marked stale (each mark refetches it).
int _staleMarks(ProviderContainer container) =>
    container.read(threadRepliesStaleProvider(_args));

void main() {
  test(
    'a live reply in a thread at its newest page shows without a request',
    () async {
      final (container, relay) = await _openThread();
      expect(relay.requests, ['room window', 'thread window']);

      relay.emit(_reply('r2', 30));
      await _settle();

      expect(relay.requests, ['room window', 'thread window']);
      expect(_staleMarks(container), 0);
      expect(_shown(container), ['r1', 'r2']);
    },
  );

  test('the echo of the only pending send folds in too', () async {
    final (container, relay) = await _openThread();
    final notifier = container.read(channelMessagesProvider(_room).notifier);
    final mine = _reply('mine', 30);
    notifier.addLocalMessage(mine);
    await _settle();
    relay.requests.clear();

    relay.emit(mine);
    await _settle();

    expect(relay.requests, isEmpty);
    expect(_shown(container), ['r1', 'mine']);
  });

  test('another send awaiting proof still marks the thread stale', () async {
    final (container, relay) = await _openThread();
    final notifier = container.read(channelMessagesProvider(_room).notifier);
    notifier.addLocalMessage(_reply('mine', 25));
    await _settle();
    relay.threadReplies = [_reply('r1', 20), _reply('r2', 30)];
    final staleBefore = _staleMarks(container);

    relay.emit(_reply('r2', 30));
    await _settle();

    // Sends awaiting proof are settled only by the complete scan it reruns.
    expect(_staleMarks(container), staleBefore + 1);
    expect(_shown(container), containsAll(['r1', 'r2']));
  });

  test('a live reply after a failed reconnect refresh refetches', () async {
    final (container, relay) = await _openThread(retry: false);
    relay.failing = true;
    relay.dropAndReconnect();
    await _settle();
    final failed = container.read(threadRepliesProvider(_args));
    expect(
      (failed.hasError, failed.isLoading, failed.hasValue),
      (true, false, true),
    );
    relay.failing = false;
    relay.requests.clear();
    final staleBefore = _staleMarks(container);

    relay.emit(_reply('r2', 30));
    await _settle();

    // The failed refresh may have left a gap under the shown replies.
    expect(_staleMarks(container), staleBefore + 1);
    expect(relay.requests, contains('thread window'));
  });

  test('a thread opened at a reply with newer pages still refetches', () async {
    final (container, relay) = await _openThread();
    final window = container.read(threadWindowProvider(_args).notifier);
    final r1 = _reply('r1', 20);
    expect(await window.openAt(createdAt: r1.createdAt, eventId: r1.id), true);
    expect(window.hasNewer, isTrue);
    relay.requests.clear();

    relay.emit(_reply('r2', 30));
    await _settle();

    expect(relay.requests, ['thread window']);
  });

  test('a thread keyed by the reply\'s parent still refetches', () async {
    final (container, relay) = await _openThread();
    const parentArgs = ThreadRepliesArgs(channelId: _room, rootId: 'r1');
    container.listen(threadRepliesProvider(parentArgs), (_, _) {});
    await container.read(threadRepliesProvider(parentArgs).future);
    relay.requests.clear();

    relay.emit(_event('nested', 30, parent: 'r1'));
    await _settle();

    // The root's window takes it; the parent's own window asks again.
    expect(relay.requests, ['thread window']);
    expect(_shown(container), ['r1', 'nested']);
  });
}
