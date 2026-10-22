import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:hooks_riverpod/misc.dart' show Override;
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channel_mutes/channel_mutes_provider.dart';
import 'package:kuzz/features/channels/channel_mutes/channel_mutes_storage.dart';
import 'package:kuzz/features/channels/channels_provider.dart';
import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/features/chats/chat_list_provider.dart';
import 'package:kuzz/features/chats/chat_row_visible_latest.dart';
import 'package:kuzz/features/chats/chats_page.dart';
import 'package:kuzz/features/chats/thread_pins_provider.dart';
import 'package:kuzz/features/people/blocked_users_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:kuzz/shared/widgets/bee_refresh_indicator.dart';
import 'package:shared_preferences/shared_preferences.dart';

const _general = '11111111-1111-4111-8111-111111111111';
const _random = '33333333-3333-4333-8333-333333333333';
const _quiet = '44444444-4444-4444-8444-444444444444';
const _group = '22222222-2222-4222-8222-222222222222';
final _them = 'b' * 64;
final _other = 'd' * 64;
final _root = 'c' * 64;
final _closedRoot = 'e' * 64;

NostrEvent _message(String id, int at, String content) => NostrEvent(
  id: id,
  pubkey: _them,
  createdAt: at,
  kind: 9,
  tags: const [],
  content: content,
);

ChatListRow _room(
  String id,
  String name,
  int at, {
  int unread = 0,
  int mentions = 0,
  bool marked = false,
  String preview = 'hello',
  String type = 'stream',
  List<String> members = const [],
}) => ChatListRow(
  isThread: false,
  key: id,
  at: at,
  atUs: at * 1000000,
  channelId: id,
  channelType: type,
  channelName: name,
  dmMembers: members,
  unread: unread,
  unreadMentions: mentions,
  unreadMark: marked,
  latest: _message('m-$id', at, preview),
);

ChatListRow _thread(
  String root,
  String title,
  int at, {
  String channelId = _general,
  int unread = 0,
  bool closed = false,
}) => ChatListRow(
  isThread: true,
  key: root,
  at: at,
  atUs: at * 1000000,
  channelId: channelId,
  channelType: 'stream',
  channelName: 'general',
  rootId: root,
  threadName: title,
  closed: closed,
  unread: unread,
  latest: _message('t-$root', at, 'thread reply'),
);

class _FakeMutes extends ChannelMutesNotifier {
  _FakeMutes(this.muted);

  final Set<String> muted;

  @override
  ChannelMutesState build() => ChannelMutesState(
    isReady: true,
    store: ChannelMuteStore(
      channels: {
        for (final id in muted)
          id: const ChannelMuteEntry(muted: true, updatedAt: 1),
      },
    ),
  );
}

class _FixedPins extends ThreadPinsNotifier {
  _FixedPins(this.pins);

  final Set<String> pins;

  @override
  Set<String> build() => pins;
}

class _FakeChannels extends ChannelsNotifier {
  _FakeChannels(this.channels, {this.refreshDone});

  final List<Channel> channels;
  final Completer<void>? refreshDone;
  int refreshes = 0;

  @override
  Future<List<Channel>> build() => SynchronousFuture(channels);

  @override
  Future<void> refresh({bool fetchDirectory = false}) {
    refreshes++;
    return refreshDone?.future ?? Future.value();
  }
}

class _FakeChatList extends ChatListNotifier {
  _FakeChatList(super.owner, this.refreshDone);

  final Completer<void> refreshDone;
  int refreshes = 0;

  @override
  Future<ChatListState> build() =>
      SynchronousFuture(const ChatListState(rows: [], next: null));

  @override
  Future<void> refresh() {
    refreshes++;
    return refreshDone.future;
  }
}

Channel _channel(String id, String name, DateTime last) => Channel(
  id: id,
  name: name,
  channelType: 'stream',
  visibility: 'open',
  description: '',
  createdBy: _them,
  createdAt: DateTime(2026),
  memberCount: 2,
  lastMessageAt: last,
  isMember: true,
);

Future<void> _pump(
  WidgetTester tester,
  List<ChatListRow> rows, {
  Set<String> muted = const {},
  List<Override> extra = const [],
}) async {
  SharedPreferences.setMockInitialValues({});
  final prefs = await SharedPreferences.getInstance();
  await tester.pumpWidget(
    ProviderScope(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        chatsRowsProvider.overrideWithValue(rows),
        channelMutesProvider.overrideWith(() => _FakeMutes(muted)),
        ...extra,
      ],
      child: MaterialApp(theme: AppTheme.light(), home: const ChatsPage()),
    ),
  );
  await tester.pump();
}

double _opacity(WidgetTester tester, String key) => tester
    .widget<Opacity>(find.byKey(ValueKey('chat-row-opacity-$key')))
    .opacity;

Color? _pillColor(WidgetTester tester, Key key) =>
    (tester
                .widget<Container>(
                  find
                      .descendant(
                        of: find.byKey(key),
                        matching: find.byType(Container),
                      )
                      .first,
                )
                .decoration
            as BoxDecoration?)
        ?.color;

void main() {
  testWidgets('time sits in a fixed right column with the counts under it', (
    tester,
  ) async {
    final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
    await _pump(tester, [
      _room(
        _general,
        'general',
        now,
        unread: 3,
        mentions: 1,
        preview: List.filled(30, 'long words').join(' '),
      ),
      _room(_random, 'random', now - 86400 * 3),
      _thread(_root, 'Release notes', now - 86400 * 4),
    ]);

    final times = [
      for (final key in [_general, _random, _root])
        tester.getRect(find.byKey(ValueKey('chat-row-time-$key'))),
    ];
    // Same right edge on every row, whatever the time's width.
    expect(times[1].right, closeTo(times[0].right, 0.01));
    expect(times[2].right, closeTo(times[0].right, 0.01));

    final count = tester.getRect(
      find.byKey(ValueKey('chat-row-unread-$_general')),
    );
    final mention = tester.getRect(
      find.byKey(ValueKey('chat-row-mention-$_general')),
    );
    expect(count.top, greaterThanOrEqualTo(times[0].bottom));
    expect(count.right, closeTo(times[0].right, 0.01));
    expect(mention.right, lessThanOrEqualTo(count.left));
    expect(
      _pillColor(tester, ValueKey('chat-row-mention-$_general')),
      const Color(0xFF0284C7),
    );

    // The preview wraps to two lines but never runs under the right column.
    final preview = tester.getRect(
      find.byKey(ValueKey('chat-row-preview-$_general')),
    );
    final meta = tester.getRect(
      find.byKey(ValueKey('chat-row-meta-$_general')),
    );
    expect(preview.right, lessThanOrEqualTo(meta.left));
    expect(
      tester
          .widget<Text>(find.byKey(ValueKey('chat-row-preview-$_general')))
          .maxLines,
      2,
    );
  });

  testWidgets('a thread in All reads #channel › thread with a two-circle '
      'avatar', (tester) async {
    await _pump(tester, [_thread(_root, 'Release notes', 100)]);

    final row = find.byKey(ValueKey('chat-row-$_root'));
    expect(
      find.descendant(of: row, matching: find.text('general')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: row, matching: find.text(' › ')),
      findsOneWidget,
    );
    expect(find.byKey(ValueKey('chat-row-title-$_root')), findsOneWidget);
    expect(find.text('Release notes'), findsOneWidget);

    final channel = tester.getRect(
      find.byKey(const ValueKey('conversation-thread-avatar-channel')),
    );
    final thread = tester.getRect(
      find.byKey(const ValueKey('conversation-thread-avatar-thread')),
    );
    expect(channel.width, 33);
    expect(thread.width, 33);
    // Side by side, overlapping, same height.
    expect(thread.left - channel.left, 18);
    expect(thread.center.dy, closeTo(channel.center.dy, 0.01));
    // The front circle and its ring stay clear of the title.
    final title = tester.getRect(
      find.descendant(of: row, matching: find.text('general')),
    );
    expect(thread.right + 2, lessThan(title.left));
    expect(find.text('G'), findsOneWidget);
    expect(find.text('R'), findsOneWidget);
  });

  testWidgets('a thread shows its group photo over the channel circle', (
    tester,
  ) async {
    final sha = 'b' * 64;
    final requested = <String>[];
    await _pump(
      tester,
      [_thread(_root, 'Release notes', 100)],
      extra: [
        channelsProvider.overrideWith(
          () => _FakeChannels([
            _channel(
              _general,
              'general',
              DateTime(2026),
            ).copyWith(picture: 'https://relay.example.com/media/$sha.png'),
          ]),
        ),
        mediaHttpClientProvider.overrideWithValue(
          http_testing.MockClient((request) async {
            requested.add(request.url.path);
            return http.Response('missing', 404);
          }),
        ),
      ],
    );
    await tester.pump();

    final back = find.byKey(
      const ValueKey('conversation-thread-avatar-channel'),
    );
    expect(
      find.descendant(
        of: back,
        matching: find.byKey(
          const ValueKey('conversation-thread-avatar-photo'),
        ),
      ),
      findsOneWidget,
    );
    // The letter stays underneath while the photo loads or fails.
    expect(find.descendant(of: back, matching: find.text('G')), findsOneWidget);
    // A 33 px circle asks for the small variant first.
    expect(requested, isNotEmpty);
    expect(requested.first, '/media/$sha.small.jpg');
  });

  testWidgets('a channel shows its letter and a group DM two people', (
    tester,
  ) async {
    await _pump(tester, [
      _room(_general, 'general', 100),
      _room(_group, 'group', 90, type: 'dm', members: [_them, _other]),
    ]);

    expect(find.byKey(const ValueKey('chat-channel-avatar')), findsOneWidget);
    expect(find.byKey(const ValueKey('chat-group-dm-avatar')), findsOneWidget);
  });

  testWidgets('a muted room counts only a mention for me (desktop rule)', (
    tester,
  ) async {
    await _pump(
      tester,
      [
        _room(_quiet, 'quiet', 100, unread: 4),
        _room(_random, 'random', 90, unread: 4, mentions: 1),
      ],
      muted: {_quiet, _random},
    );

    // Unread without a mention: dimmed, not bold, no count.
    expect(_opacity(tester, _quiet), 0.5);
    expect(find.byKey(ValueKey('chat-row-muted-$_quiet')), findsOneWidget);
    expect(find.byKey(ValueKey('chat-row-unread-$_quiet')), findsNothing);
    expect(
      tester
          .widget<Text>(find.byKey(ValueKey('chat-row-preview-$_quiet')))
          .style
          ?.fontWeight,
      isNot(FontWeight.w600),
    );
    // A mention: full strength, @ and a grey count.
    expect(_opacity(tester, _random), 1);
    expect(find.byKey(ValueKey('chat-row-mention-$_random')), findsOneWidget);
    final theme = AppTheme.light();
    expect(
      _pillColor(tester, ValueKey('chat-row-unread-$_random')),
      theme.colorScheme.onSurface.withValues(alpha: 0.2),
    );
  });

  testWidgets('a closed thread dims and shows a check', (tester) async {
    await _pump(tester, [
      _thread(_closedRoot, 'Done', 100, closed: true, unread: 2),
      _thread(_root, 'Open', 90, unread: 2),
    ]);

    expect(_opacity(tester, _closedRoot), 0.5);
    expect(
      find.byKey(ValueKey('chat-row-closed-$_closedRoot')),
      findsOneWidget,
    );
    expect(_opacity(tester, _root), 1);
    expect(find.byKey(ValueKey('chat-row-closed-$_root')), findsNothing);
    // Unread and open: the preview is bold.
    final open = tester.widget<Text>(
      find.byKey(ValueKey('chat-row-preview-$_root')),
    );
    expect(open.style?.fontWeight, FontWeight.w600);
  });

  testWidgets('dots: a marked-unread room, and a room with unread threads', (
    tester,
  ) async {
    await _pump(tester, [
      _room(_random, 'random', 100, marked: true),
      _room(_general, 'general', 90),
      _thread(_root, 'Open', 80, unread: 1),
      _thread(_closedRoot, 'Read', 70),
    ]);

    expect(find.byKey(ValueKey('chat-row-marked-$_random')), findsOneWidget);
    expect(
      find.byKey(ValueKey('chat-row-thread-unread-$_general')),
      findsOneWidget,
    );
    expect(
      find.byKey(ValueKey('chat-row-thread-unread-$_random')),
      findsNothing,
    );
    // A read thread in the same channel shows no dot of its own.
    expect(
      find.byKey(ValueKey('chat-row-thread-unread-$_closedRoot')),
      findsNothing,
    );
  });

  testWidgets('a blocked latest shows an earlier visible message or a '
      'neutral line', (tester) async {
    final earlier = NostrEvent(
      id: 'earlier',
      pubkey: _other,
      createdAt: 50,
      kind: 9,
      tags: const [],
      content: 'still visible',
    );
    await _pump(
      tester,
      [
        _room(_general, 'general', 100, preview: 'secret one'),
        _room(_random, 'random', 90, preview: 'secret two'),
      ],
      extra: [
        blockedPubkeysProvider.overrideWithValue({_them}),
        chatRowVisibleLatestProvider.overrideWith(
          (ref, key) async => key.channelId == _general ? earlier : null,
        ),
      ],
    );
    await tester.pump();

    expect(find.textContaining('secret'), findsNothing);
    expect(find.text('still visible'), findsOneWidget);
    expect(find.text(blockedMessageLabel), findsOneWidget);
  });

  testWidgets('a one-to-one DM shows what someone I blocked sent before; a '
      'group DM does not', (tester) async {
    final me = 'a' * 64;
    await _pump(
      tester,
      [
        _room(
          _general,
          'dm',
          100,
          type: 'dm',
          members: [me, _them],
          preview: 'sent before the block',
        ),
        _room(
          _group,
          'group',
          90,
          type: 'dm',
          members: [me, _them, _other],
          preview: 'secret in the group',
        ),
      ],
      extra: [
        blockedPubkeysProvider.overrideWithValue({_them}),
        chatRowVisibleLatestProvider.overrideWith((ref, key) async => null),
      ],
    );
    await tester.pump();

    expect(find.text('sent before the block'), findsOneWidget);
    expect(find.text('secret in the group'), findsNothing);
    expect(find.text(blockedMessageLabel), findsOneWidget);
  });

  testWidgets('a pinned thread shows a pin', (tester) async {
    await _pump(
      tester,
      [_thread(_root, 'Pinned', 100)],
      extra: [
        threadPinsProvider.overrideWith(() => _FixedPins({_root})),
      ],
    );

    expect(find.byKey(ValueKey('chat-row-pinned-$_root')), findsOneWidget);
  });

  testWidgets('pull to refresh waits for the Chats list only', (tester) async {
    final chatDone = Completer<void>();
    final channelsDone = Completer<void>();
    final chats = _FakeChatList('owner', chatDone);
    final channels = _FakeChannels(const [], refreshDone: channelsDone);
    await _pump(
      tester,
      [_room(_general, 'general', 100)],
      extra: [
        chatListOwnerProvider.overrideWithValue('owner'),
        chatListForOwnerProvider('owner').overrideWith(() => chats),
        channelsProvider.overrideWith(() => channels),
      ],
    );

    var finished = false;
    final indicator = tester.widget<BeeRefreshIndicator>(
      find.byType(BeeRefreshIndicator),
    );
    unawaited(indicator.onRefresh().then((_) => finished = true));
    await tester.pump();
    expect(chats.refreshes, 1);
    expect(channels.refreshes, 1);
    expect(finished, isFalse);

    chatDone.complete();
    await tester.pump();
    // Done although the channel list is still loading.
    expect(finished, isTrue);
    expect(channelsDone.isCompleted, isFalse);
  });

  group('chatsRowsProvider before the first list', () {
    ProviderContainer container(
      AsyncValue<ChatListState> list,
      List<Channel> channels,
    ) {
      final container = ProviderContainer(
        overrides: [
          chatListProvider.overrideWithValue(list),
          channelsProvider.overrideWith(() => _FakeChannels(channels)),
        ],
      );
      addTearDown(container.dispose);
      return container;
    }

    test('shows this device\'s rooms newest first while loading', () async {
      final c = container(const AsyncLoading(), [
        _channel(_general, 'general', DateTime(2026, 10, 1)),
        _channel(_random, 'random', DateTime(2026, 10, 3)),
      ]);
      await c.read(channelsProvider.future);

      final rows = c.read(chatsRowsProvider);
      expect(rows?.map((row) => row.channelId), [_random, _general]);
    });

    test('is empty (loading) with no rooms on this device either', () async {
      final c = container(const AsyncLoading(), const []);
      await c.read(channelsProvider.future);

      expect(c.read(chatsRowsProvider), isNull);
    });

    test('the relay\'s list replaces the device rooms', () async {
      final c = container(
        AsyncData(
          ChatListState(rows: [_thread(_root, 'From relay', 500)], next: null),
        ),
        [_channel(_general, 'general', DateTime(2026, 10, 1))],
      );
      await c.read(channelsProvider.future);

      final rows = c.read(chatsRowsProvider)!;
      expect(rows.first.key, _root);
      // A device room the relay did not list still follows.
      expect(rows.map((row) => row.channelId), contains(_general));
    });
  });
}
