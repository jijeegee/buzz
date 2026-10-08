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

    String fillerId(int i) => (i + 1).toRadixString(16).padLeft(64, '0');

    NostrEvent quotingOf(String quotedId, {required int createdAt}) => _textMsg(
      id: _quotingEventId,
      pubkey: _quoteBob,
      content: 'Agreed with this',
      createdAt: createdAt,
      extraTags: [
        ['q', quotedId, '', _quoteAlice],
      ],
    );

    group('tapping the quote header', () {
      late StreamController<Uri> uriLinks;

      setUp(() {
        uriLinks = StreamController<Uri>.broadcast();
        PendingDeepLinkNotifier.debugUriStreamOverride = uriLinks.stream;
      });

      tearDown(() async {
        PendingDeepLinkNotifier.debugUriStreamOverride = null;
        await uriLinks.close();
      });

      testWidgets('scrolls to a loaded channel message in place', (
        tester,
      ) async {
        await tester.pumpWidget(
          _buildTestable(
            messages: [
              quoted(),
              for (var i = 0; i < 40; i++)
                _textMsg(
                  id: fillerId(i),
                  pubkey: _quoteBob,
                  content: 'Filler $i',
                  createdAt: 1100 + i * 400,
                ),
              quotingOf(_quotedEventId, createdAt: 20000),
            ],
            users: users(),
          ),
        );
        await tester.pumpAndSettle();
        final quotedRow = find.byKey(
          ValueKey('channel-message-group-$_quotedEventId'),
        );
        expect(quotedRow, findsNothing);

        await tester.tap(
          find.byKey(ValueKey('message-quote-header-$_quotedEventId')),
        );
        await tester.pumpAndSettle();
        expect(quotedRow, findsOneWidget);
        final page = tester.getRect(find.byType(ChannelDetailPage));
        final row = tester.getRect(quotedRow);
        expect(row.top, greaterThanOrEqualTo(page.top));
        expect(row.bottom, lessThanOrEqualTo(page.bottom));
        expect(find.byType(ChannelDetailPage), findsOneWidget);
        final container = ProviderScope.containerOf(
          tester.element(find.byType(ChannelDetailPage)),
        );
        expect(container.read(pendingDeepLinkProvider), isNull);
      });

      testWidgets('opens a thread reply missing from the main timeline '
          'through the deep link', (tester) async {
        final reply = _textMsg(
          id: fillerId(0),
          pubkey: _quoteAlice,
          content: 'A thread reply',
          createdAt: 1100,
          extraTags: [
            ['e', _quotedEventId, '', 'reply'],
          ],
        );
        await tester.pumpWidget(
          _buildTestable(
            messages: [quoted(), reply, quotingOf(reply.id, createdAt: 1200)],
            users: users(),
          ),
        );
        await tester.pumpAndSettle();

        await tester.tap(
          find.byKey(ValueKey('message-quote-header-${reply.id}')),
        );
        await tester.pumpAndSettle();

        final container = ProviderScope.containerOf(
          tester.element(find.byType(ChannelDetailPage)),
        );
        expect(
          container.read(pendingDeepLinkProvider),
          MessageDeepLink(
            channelId: _channelId,
            messageId: reply.id,
            threadRootId: _quotedEventId,
          ),
        );
      });

      testWidgets('scrolls to and highlights a loaded thread reply in place', (
        tester,
      ) async {
        final root = quoted();
        final firstReplyId = fillerId(0);
        final replies = [
          for (var i = 0; i < 30; i++)
            _textMsg(
              id: fillerId(i),
              pubkey: _quoteBob,
              content: 'Reply $i',
              createdAt: 1100 + i * 10,
              extraTags: [
                ['e', _quotedEventId, '', 'reply'],
              ],
            ),
          _textMsg(
            id: _quotingEventId,
            pubkey: _quoteBob,
            content: 'Agreed with this',
            createdAt: 2000,
            extraTags: [
              ['e', _quotedEventId, '', 'reply'],
              ['q', firstReplyId, '', _quoteBob],
            ],
          ),
        ];
        await tester.pumpWidget(
          _buildTestable(
            messages: [root],
            users: users(),
            threadReplies: {_quotedEventId: replies},
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
        final targetRow = find.byKey(ValueKey('thread-message-$firstReplyId'));
        expect(targetRow.hitTestable(), findsNothing);

        await tester.tap(
          find.byKey(ValueKey('message-quote-header-$firstReplyId')),
        );
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 400));

        expect(targetRow.hitTestable(), findsOneWidget);
        Color rowColor() =>
            ((tester.widget<DecoratedBox>(targetRow).decoration
                    as BoxDecoration)
                .color ??
            Colors.transparent);
        expect(rowColor().a, greaterThan(0));
        expect(find.byType(ThreadDetailPage), findsOneWidget);
        final container = ProviderScope.containerOf(
          tester.element(find.byType(ThreadDetailPage)),
        );
        expect(container.read(pendingDeepLinkProvider), isNull);

        await tester.pump(const Duration(seconds: 4));
        await tester.pumpAndSettle();
        expect(rowColor().a, 0);
      });
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
