part of '../channel_detail_page_test.dart';

final _messageListFinder = find.byWidgetPredicate(
  (widget) => widget.runtimeType.toString() == '_MessageList',
);

/// The formatted window the page handed to its message list.
List<TimelineMessage> _shownTimeline(WidgetTester tester) =>
    (tester.widget(_messageListFinder) as dynamic).allMessages
        as List<TimelineMessage>;

/// Runs [change] and returns the type names of the widgets rebuilt for it.
Future<List<String>> _rebuiltBy(
  WidgetTester tester,
  void Function() change,
) async {
  final rebuilt = <String>[];
  final previous = debugOnRebuildDirtyWidget;
  debugOnRebuildDirtyWidget = (element, _) =>
      rebuilt.add(element.widget.runtimeType.toString());
  try {
    change();
    await tester.pumpAndSettle();
  } finally {
    debugOnRebuildDirtyWidget = previous;
  }
  return rebuilt;
}

void otherRoomRebuildTests() {
  testWidgets('another room\'s message does not rebuild the open room', (
    tester,
  ) async {
    final other = Channel(
      id: _otherChannelId,
      name: 'random',
      channelType: 'stream',
      visibility: 'open',
      description: '',
      createdBy: 'abc123',
      createdAt: DateTime(2025),
      memberCount: 3,
      isMember: true,
    );
    final channels = _FakeChannelsNotifier([_testChannel, other]);
    await tester.pumpWidget(
      _buildTestable(
        messages: [_textMsg(id: 'm1', pubkey: 'alice', content: 'hello')],
        channelsNotifier: channels,
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byType(ComposeBar), findsOneWidget);
    expect(_messageListFinder, findsOneWidget);
    final timeline = _shownTimeline(tester);

    // A live message elsewhere only advances that room's activity time.
    final otherActivity = await _rebuiltBy(tester, () {
      channels.setChannels([
        _testChannel,
        other.copyWith(lastMessageAt: DateTime.utc(2026, 10, 24)),
      ]);
    });
    expect(otherActivity, isNot(contains('ChannelDetailPage')));
    expect(otherActivity, isNot(contains('_MessageList')));
    expect(otherActivity, isNot(contains('ComposeBar')));
    expect(otherActivity, isNot(contains('MessageContent')));

    // This room's own entry still reaches the page, and a rebuild with the
    // same events reuses the formatted window.
    final ownActivity = await _rebuiltBy(tester, () {
      channels.setChannels([
        _testChannel.copyWith(lastMessageAt: DateTime.utc(2026, 10, 24)),
        other,
      ]);
    });
    expect(ownActivity, contains('ChannelDetailPage'));
    expect(identical(_shownTimeline(tester), timeline), isTrue);

    // A renamed room is a real change: the message list rebuilds for it.
    final renamed = await _rebuiltBy(tester, () {
      channels.setChannels([_testChannel, other.copyWith(name: 'renamed')]);
    });
    expect(renamed, contains('_MessageList'));
  });
}
