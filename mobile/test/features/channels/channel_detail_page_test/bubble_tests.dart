part of '../channel_detail_page_test.dart';

// The fixture profile signs in as `self` (see _FakeProfileNotifier).
const _bubbleSelf = 'self';
final _bubbleAlice = 'a' * 64;
final _bubbleAgent = 'e' * 64;

const _bubbleCodeReport =
    'Build log excerpt:\n\n```ts\nexport function computeBubbleWidth(body: string, isOwnMessage: boolean): string { return isOwnMessage ? "75%" : "92%"; }\n```';
const _bubbleTableReport =
    '| Check | Status | Notes |\n| --- | --- | --- |\n| typecheck | pass | no new errors |\n| unit tests | pass | same as main |';
const _bubbleLongReport =
    '## Release report\n\nThe build is ready for review. Your own messages now sit on the right without an avatar, while messages from other people and agents stay on the left. Consecutive messages share one avatar and name.\n\n- Installer: verified\n- Relay: verified';

/// Set to a directory to write channel/thread PNGs for visual review:
/// `flutter test --dart-define=BUZZ_BUBBLE_SHOTS=C:/tmp/shots ...`
final _appShotKey = GlobalKey(debugLabel: 'app-shot');

const _bubbleShotsDir = String.fromEnvironment('BUZZ_BUBBLE_SHOTS');

void bubbleTests() {
  group('Chat bubbles', () {
    Map<String, UserProfile> users() => {
      _bubbleAlice: UserProfile(pubkey: _bubbleAlice, displayName: 'Alice'),
      _bubbleAgent: UserProfile(
        pubkey: _bubbleAgent,
        displayName: 'My Agent',
        ownerPubkey: _bubbleSelf,
      ),
    };

    List<NostrEvent> messages() => [
      _textMsg(
        id: 'own-1',
        pubkey: _bubbleSelf,
        content: 'Can you check the deploy before lunch?',
        createdAt: 1000,
      ),
      _textMsg(
        id: 'own-2',
        pubkey: _bubbleSelf,
        content: "Also send me the table when it's ready.",
        createdAt: 1010,
      ),
      _textMsg(
        id: 'alice-1',
        pubkey: _bubbleAlice,
        content: 'Sure, looking now.',
        createdAt: 1030,
      ),
      _textMsg(
        id: 'agent-1',
        pubkey: _bubbleAgent,
        content: 'Deploy finished. Summary below.',
        createdAt: 1060,
      ),
      _textMsg(
        id: 'agent-2',
        pubkey: _bubbleAgent,
        content: _bubbleCodeReport,
        createdAt: 1062,
      ),
      _textMsg(
        id: 'agent-3',
        pubkey: _bubbleAgent,
        content: _bubbleTableReport,
        createdAt: 1064,
      ),
      _textMsg(
        id: 'agent-4',
        pubkey: _bubbleAgent,
        content: _bubbleLongReport,
        createdAt: 1066,
      ),
      _textMsg(
        id: 'thread-reply-agent',
        pubkey: _bubbleAgent,
        content: 'Scheduled for 1:30 PM.',
        createdAt: 1100,
        extraTags: const [
          ['e', 'own-1', '', 'reply'],
        ],
      ),
      _textMsg(
        id: 'thread-reply-own',
        pubkey: _bubbleSelf,
        content: 'Thanks, ship it after lunch.',
        createdAt: 1110,
        extraTags: const [
          ['e', 'own-1', '', 'reply'],
        ],
      ),
    ];

    Finder bubble(String id) => find.byKey(ValueKey('message-bubble-$id'));

    Future<void> pumpChannel(
      WidgetTester tester, {
      ThemeData? theme,
      Channel? channel,
    }) async {
      // Tall enough that the whole fixture fits without scrolling.
      tester.view.physicalSize = const Size(1170, 4800);
      tester.view.devicePixelRatio = 3;
      addTearDown(tester.view.reset);
      await _loadBubbleFonts();
      await tester.pumpWidget(
        _buildTestable(
          messages: messages(),
          users: users(),
          theme: theme,
          channel: channel,
        ),
      );
      await tester.pumpAndSettle();
    }

    testWidgets('own messages sit right without a profile; agents stay left', (
      tester,
    ) async {
      await pumpChannel(tester);
      final width = tester.getSize(find.byType(ChannelDetailPage)).width;

      final own = tester.getRect(bubble('own-2'));
      final alice = tester.getRect(bubble('alice-1'));
      expect(own.right, greaterThan(width * 0.85));
      expect(own.left, greaterThan(alice.left));
      expect(find.byKey(const ValueKey('message-author-own-1')), findsNothing);

      for (final id in ['alice-1', 'agent-1']) {
        final rect = tester.getRect(bubble(id));
        expect(rect.left, lessThan(width / 2), reason: id);
        expect(find.byKey(ValueKey('message-author-$id')), findsOneWidget);
      }
      // Grouped agent messages carry the name only on the first bubble.
      expect(
        find.byKey(const ValueKey('message-author-agent-2')),
        findsNothing,
      );
      // The reply summary follows its own message to the right.
      final summary = tester.getRect(
        find.byKey(const ValueKey('thread-summary-own-1')),
      );
      expect(summary.right, greaterThan(width * 0.85));
    });

    testWidgets('bubbles respect the 75% / 92% width caps', (tester) async {
      await pumpChannel(tester);
      final width = tester.getSize(find.byType(ChannelDetailPage)).width;
      expect(
        tester.getSize(bubble('own-1')).width,
        lessThanOrEqualTo(width * chatOwnBubbleWidthFactor),
      );
      expect(
        tester.getSize(bubble('agent-4')).width,
        lessThanOrEqualTo(width * chatOtherBubbleWidthFactor),
      );
    });

    testWidgets('wide tables still scroll sideways inside a bubble', (
      tester,
    ) async {
      await pumpChannel(tester);
      final tableScroll = find.descendant(
        of: bubble('agent-3'),
        matching: find.byWidgetPredicate(
          (widget) =>
              widget is Scrollable &&
              widget.axisDirection == AxisDirection.right,
        ),
      );
      expect(tableScroll, findsOneWidget);
      final position = tester.state<ScrollableState>(tableScroll).position;
      expect(position.maxScrollExtent, greaterThan(0));
      await tester.drag(tableScroll, const Offset(-200, 0));
      await tester.pumpAndSettle();
      expect(position.pixels, greaterThan(0));
    });

    if (_bubbleShotsDir.isNotEmpty) {
      for (final dark in [false, true]) {
        final label = dark ? 'dark' : 'light';
        testWidgets('screenshots ($label)', (tester) async {
          final theme = dark
              ? AppTheme.dark(
                  topSectionGradient: buzzTopSectionGradient(
                    buzzDarkThemeName,
                    Brightness.dark,
                  ),
                )
              : AppTheme.light(
                  topSectionGradient: buzzTopSectionGradient(
                    buzzThemeName,
                    Brightness.light,
                  ),
                );
          await pumpChannel(tester, theme: theme);
          await _writeBubbleShot(tester, 'channel-$label');

          final summary = find.byKey(const ValueKey('thread-summary-own-1'));
          await tester.ensureVisible(summary);
          await tester.pumpAndSettle();
          await tester.tap(summary);
          await tester.pumpAndSettle();
          await _writeBubbleShot(tester, 'thread-$label');
        });
        testWidgets('DM screenshot ($label)', (tester) async {
          await pumpChannel(
            tester,
            theme: dark
                ? AppTheme.dark(
                    topSectionGradient: buzzTopSectionGradient(
                      buzzDarkThemeName,
                      Brightness.dark,
                    ),
                  )
                : AppTheme.light(
                    topSectionGradient: buzzTopSectionGradient(
                      buzzThemeName,
                      Brightness.light,
                    ),
                  ),
            channel: Channel(
              id: _channelId,
              name: 'alice',
              channelType: 'dm',
              visibility: 'private',
              description: '',
              createdBy: _bubbleSelf,
              createdAt: DateTime(2025),
              memberCount: 2,
              isMember: true,
            ),
          );
          await _writeBubbleShot(tester, 'dm-$label');
        });
      }
    }
  });
}

Future<void> _loadBubbleFonts() async {
  final inter = FontLoader('Inter')
    ..addFont(rootBundle.load('assets/fonts/InterVariable.ttf'));
  final mono = FontLoader('GeistMono')
    ..addFont(rootBundle.load('assets/fonts/GeistMono-Variable.ttf'));
  await Future.wait([inter.load(), mono.load()]);
}

Future<void> _writeBubbleShot(WidgetTester tester, String name) async {
  final boundary = tester.renderObject<RenderRepaintBoundary>(
    find.byKey(_appShotKey),
  );
  await tester.runAsync(() async {
    final image = await boundary.toImage(pixelRatio: 2);
    final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
    final file = File('$_bubbleShotsDir/$name.png');
    await file.parent.create(recursive: true);
    await file.writeAsBytes(bytes!.buffer.asUint8List());
  });
}
