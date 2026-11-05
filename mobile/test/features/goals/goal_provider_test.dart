import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/goals/goal_provider.dart';
import 'package:kuzz/features/goals/goal_tree.dart';
import 'package:kuzz/shared/relay/relay.dart';

const _channel = 'room';
const _principal =
    '82e61901de54423bc233da42679d23e043de6fb51633bb274605df5c256e7b4d';

Map<String, dynamic> _row(String id, {String? parent, bool matched = true}) => {
  'id': id,
  'parent': parent,
  'title': id,
  'status': 'open',
  'matched': matched,
};

Map<String, dynamic> _page(
  List<Map<String, dynamic>> rows, {
  Object? next,
  int active = 0,
}) => {
  'goals': 1,
  'counts': {'active': active, 'inactive': 0, 'done': 0},
  'rows': rows,
  'next': next,
};

class _FakeSession extends RelaySessionNotifier {
  _FakeSession(this.answer);

  final Map<String, dynamic> Function(Map<String, dynamic> filter) answer;
  final filters = <Map<String, dynamic>>[];
  final published = <NostrEvent>[];
  Completer<void>? gate;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<Object?> queryRelayJson(
    List<Map<String, dynamic>> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    this.filters.add(filters.single);
    final held = gate;
    if (held != null && filters.single['cursor'] != null) await held.future;
    return answer(filters.single);
  }

  @override
  Future<NostrEvent> publish(
    NostrEvent event, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    published.add(event);
    return event;
  }
}

class _TokenConfig extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(
    baseUrl: 'https://relay.example',
    tokenAuth: true,
    principalId: _principal,
  );
}

ProviderContainer _container(_FakeSession session) {
  final container = ProviderContainer(
    overrides: [
      relayConfigProvider.overrideWith(_TokenConfig.new),
      relaySessionProvider.overrideWith(() => session),
    ],
  );
  addTearDown(container.dispose);
  return container;
}

void main() {
  test('the active goals read every page', () async {
    final session = _FakeSession(
      (filter) => filter['cursor'] == null
          ? _page([_row('root'), _row('a', parent: 'root')], next: 'c1')
          : _page([
              _row('root', matched: false),
              _row('b', parent: 'root'),
            ], active: 3),
    );
    final board = await _container(
      session,
    ).read(goalTreeProvider(_channel).future);

    expect(board.tree.nodes.map((node) => node.id), ['root', 'a', 'b']);
    expect(board.tree.node('root')!.matched, isTrue);
    expect(board.counts.active, 3);
    expect(session.filters.map((filter) => filter['cursor']), [null, 'c1']);
  });

  test('a thread read adds its goals to the active ones', () async {
    final session = _FakeSession(
      (filter) => filter.containsKey('threads')
          ? _page([_row('root'), _row('old', parent: 'root')])
          : _page([_row('root'), _row('a', parent: 'root')]),
    );
    final tree = await _container(session).read(
      threadGoalTreeProvider(threadGoalsKey(_channel, ['AB', 'cd'])).future,
    );

    expect(tree.nodes.map((node) => node.id), ['root', 'a', 'old']);
    expect(session.filters.last['threads'], ['ab', 'cd']);
  });

  group('a folded list', () {
    const key = (channelId: _channel, view: GoalView.done);

    Map<String, dynamic> pages(Map<String, dynamic> filter) => switch (filter) {
      {'view': 'active'} => _page([_row('root')]),
      {'cursor': 'c1'} => _page([_row('b', parent: 'root')], next: 'c2'),
      {'cursor': 'c2'} => _page([_row('c', parent: 'root')]),
      _ => _page([_row('root'), _row('a', parent: 'root')], next: 'c1'),
    };

    test('adds a page at a time', () async {
      final session = _FakeSession(pages);
      final container = _container(session);
      final subscription = container.listen(goalListProvider(key), (_, _) {});
      addTearDown(subscription.close);
      await container.read(goalListProvider(key).future);

      await container.read(goalListProvider(key).notifier).loadMore();

      final read = container.read(goalListProvider(key)).value!;
      expect(read.tree.nodes.map((node) => node.id), ['root', 'a', 'b']);
      expect(read.next, 'c2');
    });

    test('re-reads the pages it showed after an edit', () async {
      final session = _FakeSession(pages);
      final container = _container(session);
      final subscription = container.listen(goalListProvider(key), (_, _) {});
      addTearDown(subscription.close);
      await container.read(goalListProvider(key).future);
      await container.read(goalListProvider(key).notifier).loadMore();

      container.invalidate(goalTreeProvider(_channel));
      await container.read(goalTreeProvider(_channel).future);
      final read = await container.read(goalListProvider(key).future);

      expect(read.tree.nodes.map((node) => node.id), ['root', 'a', 'b']);
      expect(read.next, 'c2');
    });

    test('drops a page a re-read overtook', () async {
      final session = _FakeSession(pages);
      final container = _container(session);
      final subscription = container.listen(goalListProvider(key), (_, _) {});
      addTearDown(subscription.close);
      await container.read(goalListProvider(key).future);

      session.gate = Completer<void>();
      final loading = container.read(goalListProvider(key).notifier).loadMore();
      container.invalidate(goalTreeProvider(_channel));
      await container.read(goalTreeProvider(_channel).future);
      await container.read(goalListProvider(key).future);
      session.gate!.complete();
      await loading;

      final read = container.read(goalListProvider(key)).value!;
      expect(read.tree.nodes.map((node) => node.id), ['root', 'a']);
      expect(read.next, 'c1');
    });
  });

  test('a token account sends edits as an unsigned draft', () async {
    final session = _FakeSession((_) => _page(const []));
    final container = _container(session);

    await container.read(goalActionsProvider).applyAll(_channel, const [
      RemoveGoal(id: 'a', recursive: true),
    ]);

    final event = session.published.single;
    expect(event.kind, kindGoalOps);
    expect(event.pubkey, _principal);
    expect(event.sig, isNull);
    expect(event.tags, [
      ['h', _channel],
    ]);
    expect(jsonDecode(event.content), {
      'ops': [
        {'op': 'remove', 'id': 'a', 'recursive': true},
      ],
    });
  });
}
