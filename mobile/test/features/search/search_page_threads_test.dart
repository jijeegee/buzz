import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channel_detail_page.dart';
import 'package:kuzz/features/channels/channels_provider.dart';
import 'package:kuzz/features/channels/small_avatar.dart';
import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/features/chats/chat_list_provider.dart';
import 'package:kuzz/features/profile/profile_provider.dart';
import 'package:kuzz/features/search/recent_searches_provider.dart';
import 'package:kuzz/features/search/search_page.dart';
import 'package:kuzz/features/search/search_provider.dart';
import 'package:kuzz/shared/profile/user_cache_provider.dart';
import 'package:kuzz/shared/profile/user_profile.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:kuzz/shared/widgets/conversation_row_parts.dart';

final _general = Channel(
  id: 'general',
  name: 'general',
  channelType: 'stream',
  visibility: 'open',
  description: '',
  createdBy: 'test',
  createdAt: DateTime(2025),
  memberCount: 3,
  isMember: true,
);

final _searchTeam = Channel(
  id: 'search-team',
  name: '검색팀',
  channelType: 'stream',
  visibility: 'open',
  description: '',
  createdBy: 'test',
  createdAt: DateTime(2025),
  memberCount: 2,
  isMember: true,
);

const _threadRoot = 'root-1';

const _threadRow = ChatListRow(
  isThread: true,
  key: _threadRoot,
  at: 200,
  atUs: 200000000,
  channelId: 'general',
  channelType: 'stream',
  channelName: 'general',
  rootId: _threadRoot,
  threadName: '채널 검색 및 초대',
);

final _rows = [
  const ChatListRow(
    isThread: false,
    key: 'general',
    at: 100,
    atUs: 100000000,
    channelId: 'general',
    channelType: 'stream',
    channelName: 'general',
  ),
  _threadRow,
];

SearchHit _hit(String id, int createdAt, {bool inThread = false}) => SearchHit(
  eventId: id,
  content: '검색창을 고쳤어요',
  kind: 9,
  pubkey: 'alice',
  channelId: 'general',
  channelName: 'general',
  createdAt: createdAt,
  score: 0,
  tags: inThread
      ? const [
          ['e', _threadRoot, '', 'root'],
          ['e', _threadRoot, '', 'reply'],
        ]
      : const [],
);

Future<void> _pump(
  WidgetTester tester,
  SearchState state, {
  List<Channel>? channels,
  NavigatorObserver? observer,
}) async {
  await tester.pumpWidget(
    ProviderScope(
      overrides: [
        searchProvider.overrideWith(() => _FakeSearchNotifier(state)),
        recentSearchesProvider.overrideWith(_FakeRecentSearchesNotifier.new),
        profileProvider.overrideWith(_FakeProfileNotifier.new),
        channelsProvider.overrideWith(
          () => _FakeChannelsNotifier(channels ?? [_general]),
        ),
        chatListProvider.overrideWithValue(
          AsyncData(ChatListState(rows: _rows, next: null)),
        ),
        userCacheProvider.overrideWith(_FakeUserCacheNotifier.new),
      ],
      child: MaterialApp(
        theme: AppTheme.light(),
        navigatorObservers: [?observer],
        home: const Scaffold(body: SearchPage()),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('a thread title match is listed first, before messages', (
    tester,
  ) async {
    await _pump(
      tester,
      SearchState(query: '검색', messageResults: [_hit('message-1', 50)]),
    );

    final thread = find.byKey(const ValueKey('search-thread-row-$_threadRoot'));
    expect(thread, findsOneWidget);
    expect(
      tester
          .widget<Text>(
            find.byKey(const ValueKey('search-thread-title-$_threadRoot')),
          )
          .data,
      '채널 검색 및 초대',
    );
    expect(
      tester
          .widget<Text>(
            find.byKey(const ValueKey('search-thread-context-$_threadRoot')),
          )
          .data,
      'Thread in general',
    );
    expect(
      find.descendant(
        of: thread,
        matching: find.byType(ConversationThreadAvatar),
      ),
      findsOneWidget,
    );
    final groups = find.byKey(const ValueKey('search-section-channels'));
    final messages = find.byKey(const ValueKey('search-section-messages'));
    expect(
      tester.getTopLeft(groups).dy,
      lessThan(tester.getTopLeft(thread).dy),
    );
    expect(
      tester.getTopLeft(thread).dy,
      lessThan(tester.getTopLeft(messages).dy),
    );
  });

  testWidgets('groups rank by match: a name starting with it comes first', (
    tester,
  ) async {
    await _pump(
      tester,
      const SearchState(query: '검색'),
      channels: [_general, _searchTeam],
    );

    final team = find.byKey(const ValueKey('search-channel-row-search-team'));
    final thread = find.byKey(const ValueKey('search-thread-row-$_threadRoot'));
    expect(tester.getTopLeft(team).dy, lessThan(tester.getTopLeft(thread).dy));
  });

  testWidgets('a message in a thread shows the thread and its group', (
    tester,
  ) async {
    await _pump(
      tester,
      SearchState(
        query: '검색',
        messageResults: [
          _hit('in-thread', 60, inThread: true),
          _hit('on-timeline', 50),
        ],
      ),
    );

    expect(
      tester
          .widget<Text>(
            find.byKey(const ValueKey('search-message-channel-in-thread')),
          )
          .data,
      'general › 채널 검색 및 초대',
    );
    expect(
      find.byKey(const ValueKey('search-message-thread-avatar-in-thread')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('search-message-avatar-in-thread')),
      findsNothing,
    );

    // A main-timeline message keeps its author's picture and the group.
    expect(
      tester
          .widget<Text>(
            find.byKey(const ValueKey('search-message-channel-on-timeline')),
          )
          .data,
      'general',
    );
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('search-message-row-on-timeline')),
        matching: find.byType(SmallAvatar),
      ),
      findsOneWidget,
    );
  });

  /// Taps [rowKey] and returns the page it opens, without building it.
  Future<ChannelDetailPage> tapAndCapture(
    WidgetTester tester,
    String rowKey,
  ) async {
    final observer = _RouteRecorder();
    await _pump(
      tester,
      SearchState(
        query: '검색',
        messageResults: [_hit('in-thread', 60, inThread: true)],
      ),
      observer: observer,
    );
    final context = tester.element(find.byType(SearchPage));
    await tester.tap(find.byKey(ValueKey(rowKey)));
    final route = observer.pushed.single as MaterialPageRoute<void>;
    final page = route.builder(context) as ChannelDetailPage;
    // Leave before the pushed page builds.
    Navigator.of(context).pop();
    return page;
  }

  testWidgets('tapping a thread opens the thread', (tester) async {
    final page = await tapAndCapture(tester, 'search-thread-row-$_threadRoot');
    expect(page.channel.id, 'general');
    expect(page.initialThreadRootId, _threadRoot);
    expect(
      page.initialThreadRouteBehavior,
      InitialThreadRouteBehavior.replaceCurrentRoute,
    );
    expect(page.initialMessageId, isNull);
  });

  testWidgets('tapping a message in a thread opens it in the thread', (
    tester,
  ) async {
    final page = await tapAndCapture(tester, 'search-message-row-in-thread');
    expect(page.initialThreadRootId, _threadRoot);
    expect(page.initialMessageId, 'in-thread');
  });
}

class _RouteRecorder extends NavigatorObserver {
  final pushed = <Route<dynamic>>[];

  @override
  void didPush(Route<dynamic> route, Route<dynamic>? previousRoute) {
    if (previousRoute != null) pushed.add(route);
  }
}

class _FakeSearchNotifier extends SearchNotifier {
  _FakeSearchNotifier(this.initialState);

  final SearchState initialState;

  @override
  SearchState build() => initialState;

  @override
  void search(String query) {}
}

class _FakeRecentSearchesNotifier extends RecentSearchesNotifier {
  @override
  List<String> build() => const [];

  @override
  void record(String query) {}
}

class _FakeProfileNotifier extends ProfileNotifier {
  @override
  Future<UserProfile?> build() async =>
      const UserProfile(pubkey: 'test', displayName: 'Test');
}

class _FakeChannelsNotifier extends ChannelsNotifier {
  _FakeChannelsNotifier(this.channels);

  final List<Channel> channels;

  @override
  Future<List<Channel>> build() async => channels;
}

class _FakeUserCacheNotifier extends UserCacheNotifier {
  @override
  Map<String, UserProfile> build() => const {
    'alice': UserProfile(pubkey: 'alice', displayName: 'Alice'),
  };
}
