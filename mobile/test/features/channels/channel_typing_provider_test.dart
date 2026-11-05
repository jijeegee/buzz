import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel_typing_provider.dart';
import 'package:kuzz/features/goals/goal_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';

void main() {
  const room = '11111111-1111-4111-8111-111111111111';

  late _RoomRequests requests;
  late _Session session;

  ProviderContainer container() {
    requests = _RoomRequests();
    session = _Session();
    final container = ProviderContainer(
      overrides: [
        relaySessionProvider.overrideWith(() => session),
        roomLiveRegistryProvider.overrideWithValue(
          RoomLiveRegistry(requests.open),
        ),
      ],
    );
    addTearDown(container.dispose);
    return container;
  }

  test('typing rides the open-room REQ; leaving closes it', () async {
    final c = container();
    final watch = c.listen(channelTypingProvider(room), (_, _) {});
    await Future<void>.delayed(Duration.zero);
    expect(requests.opened, hasLength(1));
    expect(requests.opened.single.filters.first.tags['#h'], [room]);

    requests.emit(_event(EventKind.typingIndicator));
    expect(c.read(channelTypingProvider(room)), hasLength(1));

    watch.close();
    await c.pump();
    await Future<void>.delayed(Duration.zero);
    expect(requests.closed, 1);
  });

  test('a reconnect neither closes nor re-sends the typing REQ', () async {
    final c = container();
    c.listen(relaySessionProvider, (_, _) {});
    c.listen(channelTypingProvider(room), (_, _) {});
    await Future<void>.delayed(Duration.zero);
    session.setStatus(SessionStatus.reconnecting);
    await c.pump();
    session.setStatus(SessionStatus.connected);
    await c.pump();
    await Future<void>.delayed(Duration.zero);
    expect(requests.opened, hasLength(1));
    expect(requests.closed, 0);
  });

  test('goal chips share the REQ, re-read on a new edit, and leaving the '
      'room closes it even after the goals page read the tree', () async {
    final c = container();
    final typing = c.listen(channelTypingProvider(room), (_, _) {});
    final goalsLive = c.listen(goalTreeLiveProvider(room), (_, _) {});
    final tree = c.listen(goalTreeProvider(room), (_, _) {});
    await c.read(goalTreeProvider(room).future);
    await Future<void>.delayed(Duration.zero);
    expect(requests.opened, hasLength(1), reason: 'one REQ for both kinds');
    final readsBefore = session.queries;

    requests.emit(_event(roomLiveGoalOpsKind));
    await c.read(goalTreeProvider(room).future);
    expect(session.queries, readsBefore + 1, reason: 'a new edit re-reads');

    typing.close();
    goalsLive.close();
    await c.pump();
    await Future<void>.delayed(Duration.zero);
    expect(requests.closed, 1, reason: 'the goals page keeps no REQ open');
    tree.close();
  });
}

NostrEvent _event(int kind) => NostrEvent(
  id: 'e$kind',
  pubkey: 'alice',
  createdAt: 1,
  kind: kind,
  tags: [
    ['h', '11111111-1111-4111-8111-111111111111'],
  ],
  content: kind == roomLiveGoalOpsKind ? '{}' : '',
  sig: 'sig',
);

class _OpenedRequest {
  _OpenedRequest(this.filters, this.onEvent);

  final List<NostrFilter> filters;
  final void Function(NostrEvent) onEvent;
}

class _RoomRequests {
  final opened = <_OpenedRequest>[];
  var closed = 0;

  Future<void Function()> open(
    List<NostrFilter> filters,
    void Function(NostrEvent) onEvent,
    void Function() onGone,
  ) async {
    opened.add(_OpenedRequest(filters, onEvent));
    return () => closed++;
  }

  void emit(NostrEvent event) {
    for (final request in List.of(opened)) {
      request.onEvent(event);
    }
  }
}

class _Session extends RelaySessionNotifier {
  var queries = 0;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  void setStatus(SessionStatus status) => state = SessionState(status: status);

  @override
  Future<Object?> queryRelayJson(
    List<Map<String, dynamic>> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    queries++;
    return const <String, dynamic>{};
  }
}
