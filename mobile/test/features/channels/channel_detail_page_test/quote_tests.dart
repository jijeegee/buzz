part of '../channel_detail_page_test.dart';

final _quoteAlice = 'a' * 64;
final _quoteBob = 'b' * 64;
final _quotedEventId = 'c' * 64;
final _quotingEventId = 'd' * 64;

void quoteTests() {
  group('Quote', () {
    Map<String, UserProfile> users() => {
      _quoteAlice: UserProfile(pubkey: _quoteAlice, displayName: 'Alice'),
      _quoteBob: UserProfile(pubkey: _quoteBob, displayName: 'Bob'),
    };

    NostrEvent quoted() => _textMsg(
      id: _quotedEventId,
      pubkey: _quoteAlice,
      content: 'The original message',
    );

    testWidgets('quoting a main-timeline message fills the main composer', (
      tester,
    ) async {
      await tester.pumpWidget(
        _buildTestable(messages: [quoted()], users: users()),
      );
      await tester.pumpAndSettle();

      await tester.longPress(findRichText('The original message'));
      await tester.pumpAndSettle();
      expect(find.text('Reply in thread'), findsOneWidget);
      await tester.tap(find.text('Quote'));
      await tester.pumpAndSettle();

      expect(find.byKey(const ValueKey('composer-quote-chip')), findsOneWidget);
      expect(find.text('Quoting Alice'), findsOneWidget);

      await tester.tap(find.byTooltip('Cancel quote'));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('composer-quote-chip')), findsNothing);
    });

    testWidgets('archived channels do not offer Quote', (tester) async {
      await tester.pumpWidget(
        _buildTestable(
          messages: [quoted()],
          users: users(),
          channel: Channel(
            id: _channelId,
            name: 'general',
            channelType: 'stream',
            visibility: 'open',
            description: 'General discussion',
            createdBy: 'abc123',
            createdAt: DateTime(2025),
            memberCount: 5,
            isMember: true,
            archivedAt: DateTime(2025, 2),
          ),
        ),
      );
      await tester.pumpAndSettle();

      await tester.longPress(findRichText('The original message'));
      await tester.pumpAndSettle();

      expect(find.text('Copy link'), findsOneWidget);
      expect(find.text('Quote'), findsNothing);

      // Dismiss so the popover's in-flight guard does not leak across tests.
      Navigator.of(
        tester.element(find.byKey(const ValueKey('message-action-surface'))),
      ).pop();
      await tester.pumpAndSettle();
    });

    testWidgets('a message with a q tag shows the quoted header', (
      tester,
    ) async {
      final quoting = _textMsg(
        id: _quotingEventId,
        pubkey: _quoteBob,
        content: 'Agreed with this',
        createdAt: 1100,
        extraTags: [
          ['q', _quotedEventId, '', _quoteAlice],
        ],
      );
      await tester.pumpWidget(
        _buildTestable(messages: [quoted(), quoting], users: users()),
      );
      await tester.pumpAndSettle();

      final header = find.byKey(
        ValueKey('message-quote-header-$_quotedEventId'),
      );
      expect(header, findsOneWidget);
      expect(
        find.descendant(of: header, matching: find.text('Alice')),
        findsOneWidget,
      );
      expect(
        find.descendant(
          of: header,
          matching: find.text('The original message'),
        ),
        findsOneWidget,
      );
    });

    testWidgets('quoting inside a thread fills the thread composer', (
      tester,
    ) async {
      final root = quoted();
      final reply = _textMsg(
        id: _quotingEventId,
        pubkey: _quoteBob,
        content: 'A thread reply',
        createdAt: 1100,
        extraTags: [
          ['e', _quotedEventId, '', 'reply'],
        ],
      );
      await tester.pumpWidget(
        _buildTestable(
          messages: [root],
          users: users(),
          threadReplies: {
            _quotedEventId: [reply],
          },
        ),
      );
      await tester.pumpAndSettle();
      final threadHead = formatTimeline([root]).single;
      Navigator.of(tester.element(find.byType(ChannelDetailPage))).push(
        MaterialPageRoute<void>(
          builder: (_) => ThreadDetailPage(
            threadHead: threadHead,
            allMessages: [threadHead],
            channelId: _channelId,
            currentPubkey: 'self',
            isMember: true,
            isArchived: false,
          ),
        ),
      );
      await tester.pumpAndSettle();

      await tester.longPress(findRichText('A thread reply'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Quote'));
      await tester.pumpAndSettle();

      expect(find.text('Quoting Bob'), findsOneWidget);
      expect(
        find.descendant(
          of: find.byType(ThreadDetailPage),
          matching: find.byKey(const ValueKey('composer-quote-chip')),
        ),
        findsOneWidget,
      );
    });
  });
}
