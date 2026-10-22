import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/features/chats/chats_page.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:shared_preferences/shared_preferences.dart';

const _channel = '11111111-1111-4111-8111-111111111111';
const _dm = '22222222-2222-4222-8222-222222222222';
final _them = 'b' * 64;

NostrEvent _message(String id, int at, String content) => NostrEvent(
  id: id,
  pubkey: _them,
  createdAt: at,
  kind: 9,
  tags: const [],
  content: content,
);

ChatListRow _thread(int index, {int unread = 0}) {
  final root = '$index' * 64;
  return ChatListRow(
    isThread: true,
    key: root,
    at: 100 - index,
    atUs: (100 - index) * 1000000,
    channelId: _channel,
    channelType: 'stream',
    channelName: 'general',
    rootId: root,
    threadName: 'Thread $index',
    unread: unread,
    latest: _message('l$index', 100 - index, 'reply $index'),
  );
}

void main() {
  final rows = [
    ChatListRow(
      isThread: false,
      key: _dm,
      at: 200,
      atUs: 200000000,
      channelId: _dm,
      channelType: 'dm',
      channelName: 'Mina',
      dmMembers: [_them],
      unread: 3,
      latest: _message('dm', 200, 'see you soon'),
    ),
    for (var i = 1; i <= 4; i++) _thread(i, unread: i == 4 ? 2 : 0),
  ];

  Future<void> pumpChats(
    WidgetTester tester, {
    List<ChatListRow>? chatRows,
  }) async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          savedPrefsProvider.overrideWithValue(prefs),
          chatsRowsProvider.overrideWithValue(chatRows ?? rows),
        ],
        child: MaterialApp(theme: AppTheme.light(), home: const ChatsPage()),
      ),
    );
    await tester.pump();
  }

  testWidgets('All lists rooms and threads newest first with unread', (
    tester,
  ) async {
    await pumpChats(tester);

    expect(find.text('see you soon'), findsOneWidget);
    expect(find.byKey(ValueKey('chat-row-unread-$_dm')), findsOneWidget);
    // All names a thread with its channel, as desktop does.
    expect(find.text('Thread 1'), findsOneWidget);
    expect(
      find.descendant(
        of: find.byKey(ValueKey('chat-row-${'1' * 64}')),
        matching: find.text('general'),
      ),
      findsOneWidget,
    );
    final dmTop = tester.getTopLeft(find.byKey(ValueKey('chat-row-$_dm'))).dy;
    final threadTop = tester
        .getTopLeft(find.byKey(ValueKey('chat-row-${'1' * 64}')))
        .dy;
    expect(dmTop, lessThan(threadTop));
  });

  testWidgets('By channel folds threads past three and expands in place', (
    tester,
  ) async {
    await pumpChats(tester);

    await tester.tap(find.byKey(const ValueKey('chats-view-byChannel')));
    await tester.pumpAndSettle();

    expect(find.text('Thread 1'), findsOneWidget);
    expect(find.text('Thread 2'), findsOneWidget);
    expect(find.text('Thread 3'), findsOneWidget);
    expect(find.text('Thread 4'), findsNothing);
    final more = find.byKey(const ValueKey('chat-more-$_channel'));
    expect(more, findsOneWidget);

    await tester.tap(more);
    await tester.pumpAndSettle();

    expect(find.text('Thread 4'), findsOneWidget);
    expect(find.text('Show less'), findsOneWidget);
    expect(find.textContaining('more'), findsNothing);
  });

  testWidgets('By channel shows 10 more threads per tap, then Show less', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 4000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    // 28 threads: 3 shown, 25 hidden; the oldest has 3 unread.
    await pumpChats(
      tester,
      chatRows: [
        for (var i = 1; i <= 28; i++) _thread(i, unread: i == 28 ? 3 : 0),
      ],
    );
    await tester.tap(find.byKey(const ValueKey('chats-view-byChannel')));
    await tester.pumpAndSettle();
    final show = find.byKey(const ValueKey('chat-more-show-$_channel'));
    final less = find.byKey(const ValueKey('chat-more-less-$_channel'));
    int threads() =>
        find.textContaining(RegExp(r'^Thread \d+$')).evaluate().length;

    expect(threads(), 3);
    expect(find.text('+ 25 more threads'), findsOneWidget);
    expect(find.descendant(of: show, matching: find.text('3')), findsOneWidget);
    expect(less, findsNothing);

    await tester.tap(show);
    await tester.pumpAndSettle();
    expect(threads(), 13);
    expect(find.text('+ 10 more · 15 left'), findsOneWidget);
    expect(find.descendant(of: show, matching: find.text('3')), findsOneWidget);
    expect(
      find.descendant(of: less, matching: find.text('Show less')),
      findsOneWidget,
    );

    await tester.tap(show);
    await tester.pumpAndSettle();
    expect(threads(), 23);
    expect(find.text('+ 5 more'), findsOneWidget);

    await tester.tap(show);
    await tester.pumpAndSettle();
    expect(threads(), 28);
    expect(show, findsNothing);
    expect(find.text('Show less'), findsOneWidget);

    await tester.tap(less);
    await tester.pumpAndSettle();
    expect(threads(), 3);
    expect(find.text('+ 25 more threads'), findsOneWidget);
  });

  testWidgets('a chat that is not loaded yet says so instead of opening', (
    tester,
  ) async {
    await pumpChats(tester);

    await tester.tap(find.byKey(ValueKey('chat-row-tap-$_dm')));
    await tester.pump();

    expect(find.text('This chat is still loading.'), findsOneWidget);
  });
}
