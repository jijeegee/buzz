import 'package:buzz/features/activity/activity_provider.dart';
import 'package:buzz/features/activity/feed_item.dart';
import 'package:buzz/features/activity/inbox_summary_provider.dart';
import 'package:buzz/features/channels/channel.dart';
import 'package:buzz/features/channels/channel_live_events.dart';
import 'package:buzz/features/channels/channels_provider.dart';
import 'package:buzz/shared/read_state/read_state_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:hooks_riverpod/misc.dart';

const _roomA = '11111111-1111-4111-8111-111111111111';
const _roomB = '22222222-2222-4222-8222-222222222222';

/// Answers summary requests from [pages] in order and records every request.
class _SummarySession extends RelaySessionNotifier {
  _SummarySession(this.pages);

  final List<Object?> pages;
  final List<Map<String, dynamic>> summaryRequests = [];
  int legacyQueries = 0;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<Object?> queryRelayJson(
    List<Map<String, dynamic>> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    summaryRequests.add(filters.single);
    if (pages.isEmpty) throw StateError('no page left');
    return pages.removeAt(0);
  }

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    legacyQueries++;
    return const [];
  }
}

class _FixedRelayConfigNotifier extends RelayConfigNotifier {
  @override
  RelayConfig build() =>
      const RelayConfig(baseUrl: 'https://relay.example', nsec: null);
}

class _FixedChannelsNotifier extends ChannelsNotifier {
  @override
  Future<List<Channel>> build() async => [_channel(_roomA), _channel(_roomB)];
}

class _EmptyActivityNotifier extends ActivityNotifier {
  @override
  Future<HomeFeedResponse> build() async => HomeFeedResponse(
    mentions: const [],
    needsAction: const [],
    activity: const [],
    agentActivity: const [],
  );
}

class _ReadStateNotifier extends ReadStateNotifier {
  @override
  ReadStateState build() => ReadStateState(
    isReady: true,
    pubkey: 'me',
    contexts: Map.unmodifiable({_roomA: 50, 'msg:x': 10}),
    version: 1,
  );
}

class _LifecycleNotifier extends AppLifecycleNotifier {
  @override
  AppLifecycleState build() => AppLifecycleState.resumed;

  void set(AppLifecycleState next) => state = next;
}

Channel _channel(String id) => Channel(
  id: id,
  name: id,
  channelType: 'stream',
  visibility: 'open',
  description: '',
  createdBy: 'x',
  createdAt: DateTime(2025),
  memberCount: 2,
  isMember: true,
);

Map<String, dynamic> _eventJson(String id, String channel, int createdAt) => {
  'id': id,
  'pubkey': 'other',
  'created_at': createdAt,
  'kind': 9,
  'tags': [
    ['h', channel],
  ],
  'content': id,
};

Map<String, dynamic> _row(String channel, int at, String latestId) => {
  'type': 'room',
  'channel_id': channel,
  'channel_type': 'stream',
  'root_id': null,
  'at': at,
  'at_us': at * 1000000,
  'key': channel,
  'reply_count': null,
  'unread': 1,
  'unread_overflow': false,
  'latest': _eventJson(latestId, channel, at),
  'root': null,
};

Map<String, dynamic> _page(
  List<Map<String, dynamic>> rows, {
  Map<String, dynamic>? next,
}) => {'inbox_summary': 1, 'rows': rows, 'next': next};

Future<void> _waitFor(bool Function() predicate) async {
  for (var attempt = 0; attempt < 100; attempt++) {
    if (predicate()) return;
    await Future<void>.delayed(const Duration(milliseconds: 10));
  }
  fail('Condition was not reached before timeout');
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  ({ProviderContainer container, _LifecycleNotifier lifecycle}) setUp(
    _SummarySession session, {
    List<Override> overrides = const [],
    Duration minReloadInterval = Duration.zero,
  }) {
    final lifecycle = _LifecycleNotifier();
    final container = ProviderContainer(
      overrides: [
        relayConfigProvider.overrideWith(_FixedRelayConfigNotifier.new),
        myPubkeyProvider.overrideWithValue('me'),
        relaySessionProvider.overrideWith(() => session),
        channelsProvider.overrideWith(_FixedChannelsNotifier.new),
        activityProvider.overrideWith(_EmptyActivityNotifier.new),
        readStateProvider.overrideWith(_ReadStateNotifier.new),
        appLifecycleProvider.overrideWith(() => lifecycle),
        inboxSummaryMinReloadIntervalProvider.overrideWithValue(
          minReloadInterval,
        ),
        ...overrides,
      ],
    );
    addTearDown(container.dispose);
    final subscription = container.listen(
      conversationInboxItemsProvider,
      (_, _) {},
    );
    addTearDown(subscription.close);
    return (container: container, lifecycle: lifecycle);
  }

  test('one summary request with read markers feeds the conversation rows; '
      'later pages load in the background', () async {
    final session = _SummarySession([
      _page(
        [_row(_roomB, 200, 'b-latest')],
        next: {'at_us': 200000000, 'key': _roomB},
      ),
      _page([_row(_roomA, 100, 'a-latest')]),
    ]);
    final (:container, lifecycle: _) = setUp(session);
    await container.read(channelsProvider.future);
    await container.read(inboxSummaryProvider.future);
    await _waitFor(() => session.summaryRequests.length == 2);
    await _waitFor(
      () => container.read(conversationInboxItemsProvider).length == 2,
    );

    final first = session.summaryRequests.first;
    expect(first['inbox_summary'], 1);
    expect(first['limit'], 50);
    expect(first['read'], {_roomA: 50});
    expect(first, isNot(contains('before')));
    expect(session.summaryRequests.last['before'], {
      'at_us': 200000000,
      'key': _roomB,
    });
    expect(session.legacyQueries, 0);
    expect(container.read(conversationInboxItemsProvider).map((r) => r.id), [
      'b-latest',
      'a-latest',
    ]);
  });

  test('a live message patches its row without another request', () async {
    final session = _SummarySession([
      _page([_row(_roomB, 200, 'b-latest'), _row(_roomA, 100, 'a-latest')]),
    ]);
    final (:container, lifecycle: _) = setUp(session);
    await container.read(channelsProvider.future);
    await container.read(inboxSummaryProvider.future);

    container
        .read(channelLiveEventsProvider)
        .emit(NostrEvent.fromJson(_eventJson('a-new', _roomA, 300)));

    final rows = container.read(conversationInboxItemsProvider);
    expect(rows.map((row) => row.id), ['a-new', 'b-latest']);
    expect(
      container.read(inboxSummaryUnreadProvider)['channel:$_roomA']?.count,
      2,
    );
    expect(session.summaryRequests, hasLength(1));
  });

  test(
    'deleting a shown message and resuming the app reload the summary',
    () async {
      final session = _SummarySession([
        _page([_row(_roomA, 100, 'a-latest')]),
        _page([_row(_roomA, 90, 'a-older')]),
        _page([_row(_roomA, 90, 'a-older')]),
      ]);
      final (:container, :lifecycle) = setUp(session);
      await container.read(channelsProvider.future);
      await container.read(inboxSummaryProvider.future);

      // A reaction removal targets no shown row.
      container
          .read(channelLiveEventsProvider)
          .emit(
            NostrEvent(
              id: 'unrelated-delete',
              pubkey: 'other',
              createdAt: 150,
              kind: EventKind.deletion,
              tags: const [
                ['h', _roomA],
                ['e', 'some-reaction'],
              ],
              content: '',
            ),
          );
      await Future<void>.delayed(Duration.zero);
      expect(session.summaryRequests, hasLength(1));

      container
          .read(channelLiveEventsProvider)
          .emit(
            NostrEvent(
              id: 'delete',
              pubkey: 'other',
              createdAt: 150,
              kind: EventKind.deletion,
              tags: const [
                ['h', _roomA],
                ['e', 'a-latest'],
              ],
              content: '',
            ),
          );
      await _waitFor(() => session.summaryRequests.length == 2);
      await _waitFor(
        () =>
            container.read(conversationInboxItemsProvider).firstOrNull?.id ==
            'a-older',
      );

      // A notification shade or dialog (inactive → resumed) reloads nothing.
      lifecycle.set(AppLifecycleState.inactive);
      lifecycle.set(AppLifecycleState.resumed);
      await Future<void>.delayed(const Duration(milliseconds: 50));
      expect(session.summaryRequests, hasLength(2));

      for (final next in const [
        AppLifecycleState.inactive,
        AppLifecycleState.hidden,
        AppLifecycleState.paused,
        AppLifecycleState.hidden,
        AppLifecycleState.inactive,
        AppLifecycleState.resumed,
      ]) {
        lifecycle.set(next);
      }
      await _waitFor(() => session.summaryRequests.length == 3);
    },
  );

  test('resume reloads wait for the minimum interval and coalesce', () async {
    final session = _SummarySession([
      _page([_row(_roomA, 100, 'a-latest')]),
      _page([_row(_roomA, 100, 'a-latest')]),
    ]);
    final (:container, :lifecycle) = setUp(
      session,
      minReloadInterval: const Duration(milliseconds: 300),
    );
    await container.read(channelsProvider.future);
    await container.read(inboxSummaryProvider.future);

    for (var i = 0; i < 3; i++) {
      lifecycle.set(AppLifecycleState.paused);
      lifecycle.set(AppLifecycleState.resumed);
    }
    await Future<void>.delayed(const Duration(milliseconds: 100));
    expect(session.summaryRequests, hasLength(1));
    await _waitFor(() => session.summaryRequests.length == 2);
    await Future<void>.delayed(const Duration(milliseconds: 400));
    expect(session.summaryRequests, hasLength(2));
  });

  test(
    'falls back to per-room loading when the relay has no summary',
    () async {
      var perRoomLoads = 0;
      final session = _SummarySession([<Object?>[]]);
      final (:container, lifecycle: _) = setUp(
        session,
        overrides: [
          channelActivityProvider.overrideWith((ref) async {
            perRoomLoads++;
            return [
              FeedItem(
                id: 'legacy',
                kind: 9,
                pubkey: 'other',
                content: 'legacy',
                createdAt: 100,
                channelId: _roomA,
                channelName: '',
                tags: const [
                  ['h', _roomA],
                ],
                category: 'activity',
              ),
            ];
          }),
        ],
      );
      await container.read(channelsProvider.future);
      await expectLater(
        container.read(inboxSummaryProvider.future),
        throwsA(anything),
      );
      await _waitFor(
        () => container.read(conversationInboxItemsProvider).isNotEmpty,
      );

      expect(inboxSummaryFailed(container.read(inboxSummaryProvider)), isTrue);
      expect(perRoomLoads, 1);
      expect(session.summaryRequests, hasLength(1));
      expect(
        container.read(conversationInboxItemsProvider).single.id,
        'legacy',
      );
    },
  );
}
