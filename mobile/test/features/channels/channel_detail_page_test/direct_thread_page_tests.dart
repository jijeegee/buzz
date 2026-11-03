part of '../channel_detail_page_test.dart';

void directThreadPageTests() {
  final reveal = find.byKey(const ValueKey('thread-open-reveal'));
  final loading = find.byKey(const ValueKey('thread-replies-loading'));

  bool listVisible(WidgetTester tester) =>
      tester.widget<Opacity>(reveal).opacity > 0;

  void useTallView(WidgetTester tester) {
    tester.view.physicalSize = const Size(400, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
  }

  Override rootLookup({
    Future<List<NostrEvent>> Function()? relay,
    Future<({NostrEvent event, bool deleted})?> Function(String id)? stored,
  }) => threadRootSourcesProvider.overrideWithValue(
    ThreadRootSources(
      stored: (_, args) => stored?.call(args.rootId) ?? Future.value(),
      relay: (_, _) => relay?.call() ?? Completer<List<NostrEvent>>().future,
    ),
  );

  Override storedReplies(Future<List<NostrEvent>> Function() events) =>
      paintableThreadHeadProvider.overrideWithValue(
        (_, _) async => serveThreadWindow(
          _channelId,
          _openAtRootId,
          await events(),
          NostrFilter(kinds: const [], limit: 50),
        ),
      );

  Widget harness(
    Widget home, {
    required RelaySessionNotifier session,
    List<Override> overrides = const [],
    RelayInfo? relayInfo,
  }) => _buildTestable(
    messages: const [],
    relaySessionNotifier: session,
    huddleCurrentPubkey: 'self',
    relayInfo: relayInfo,
    channelsNotifier: _FakeChannelsNotifier([_testChannel]),
    readStateNotifier: _SynchronousReadStateNotifier(
      const ReadStateState(
        isReady: true,
        pubkey: 'self',
        contexts: {_channelId: 5000},
        version: 0,
      ),
    ),
    overrides: overrides,
    home: home,
  );

  ThreadDetailPage threadPage({
    String? initialMessageId,
    int? initialMessageCreatedAt,
    bool awaitsLinkedReply = false,
    bool headMayBeNested = false,
  }) => ThreadDetailPage(
    headId: _openAtRootId,
    channelId: _channelId,
    currentPubkey: 'self',
    isMember: true,
    isArchived: false,
    channelBelow: false,
    initialMessageId: initialMessageId,
    initialMessageCreatedAt: initialMessageCreatedAt,
    awaitsLinkedReply: awaitsLinkedReply,
    headMayBeNested: headMayBeNested,
  );

  group('The thread page without its relay', () {
    final replies = [for (var n = 100; n < 160; n++) _openAtReply(n)];
    final windowRelay = RelayInfo(supportedExtensions: {newerWindowExtension});

    testWidgets('a short stored thread shows its name, never loading text', (
      tester,
    ) async {
      await tester.pumpWidget(
        harness(
          threadPage(),
          session: _SilentRelaySession(),
          overrides: [
            rootLookup(),
            storedReplies(() async => replies.take(3).toList()),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();

      expect(findRichText('row 102'), findsOneWidget);
      expect(find.byKey(const ValueKey('thread-root-pending')), findsOneWidget);
      expect(loading, findsNothing);
    });

    testWidgets('loading text waits for the store, then shows at a link', (
      tester,
    ) async {
      final store = Completer<List<NostrEvent>>();
      await tester.pumpWidget(
        harness(
          threadPage(initialMessageId: fixtureId(120)),
          session: _SilentRelaySession(),
          relayInfo: windowRelay,
          overrides: [rootLookup(), storedReplies(() => store.future)],
        ),
      );
      await tester.pump();
      await tester.pump();

      expect(loading, findsNothing);

      store.complete(const []);
      await tester.pump();
      await tester.pump();

      expect(listVisible(tester), isTrue);
      expect(loading, findsOneWidget);
    });

    testWidgets('a linked reply a drain brings opens there', (tester) async {
      useTallView(tester);
      var stored = const <NostrEvent>[];
      await tester.pumpWidget(
        harness(
          threadPage(initialMessageId: fixtureId(120)),
          session: _SilentRelaySession(),
          relayInfo: windowRelay,
          overrides: [rootLookup(), storedReplies(() async => stored)],
        ),
      );
      await tester.pump();
      await tester.pump();
      expect(findRichText('row 120'), findsNothing);

      stored = replies;
      ProviderScope.containerOf(
        tester.element(find.byType(ThreadDetailPage)),
      ).read(pushInboxRevisionProvider.notifier).bump();
      await tester.pump();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));

      expect(listVisible(tester), isTrue);
      expect(findRichText('row 120'), findsOneWidget);
      expect(findRichText('row 159'), findsNothing);
    });

    testWidgets('offline, a store slower than the relay check still lands', (
      tester,
    ) async {
      useTallView(tester);
      final store = Completer<List<NostrEvent>>();
      await tester.pumpWidget(
        harness(
          threadPage(initialMessageId: fixtureId(120)),
          session: _SilentRelaySession(),
          overrides: [
            relayInfoProvider.overrideWith((ref) async => null),
            rootLookup(),
            storedReplies(() => store.future),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();
      await tester.pump();

      store.complete(replies);
      await tester.pump();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));

      expect(listVisible(tester), isTrue);
      expect(findRichText('row 120'), findsOneWidget);
      expect(findRichText('row 159'), findsNothing);
    });

    testWidgets('a draft head found in the store opens the composer', (
      tester,
    ) async {
      await tester.pumpWidget(
        harness(
          threadPage(headMayBeNested: true),
          session: _SilentRelaySession(),
          overrides: [
            rootLookup(
              stored: (id) async => (event: _openAtRoot, deleted: false),
            ),
            storedReplies(() async => const []),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();

      expect(find.byType(ComposeBar), findsOneWidget);
    });

    testWidgets('a draft head waits for the lookup while it is out', (
      tester,
    ) async {
      await tester.pumpWidget(
        harness(
          threadPage(headMayBeNested: true),
          session: _SilentRelaySession(),
          overrides: [rootLookup(), storedReplies(() async => const [])],
        ),
      );
      await tester.pump();
      await tester.pump();

      expect(find.byType(ComposeBar), findsNothing);
    });

    testWidgets('a failed lookup never hides the composer', (tester) async {
      await tester.pumpWidget(
        harness(
          threadPage(headMayBeNested: true),
          session: _SilentRelaySession(),
          overrides: [
            rootLookup(relay: () async => throw TimeoutException('root')),
            storedReplies(() async => const []),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();
      await tester.pump();

      expect(find.byType(ComposeBar), findsOneWidget);
    });
  });

  group('A linked reply beyond the walked pages', () {
    final replies = [for (var n = 100; n < 900; n++) _openAtReply(n)];
    final target = findRichText('row 110');

    Widget linked(
      _ThreadWindowRelaySession session,
      ValueNotifier<int?> createdAt, {
      required bool awaitsWhileUnknown,
    }) => harness(
      ValueListenableBuilder(
        valueListenable: createdAt,
        builder: (_, at, _) => threadPage(
          initialMessageId: fixtureId(110),
          initialMessageCreatedAt: at,
          awaitsLinkedReply: awaitsWhileUnknown && at == null,
        ),
      ),
      session: session,
      relayInfo: RelayInfo(supportedExtensions: {newerWindowExtension}),
      overrides: [
        rootLookup(relay: () async => [_openAtRoot]),
        storedReplies(() async => const []),
      ],
    );

    testWidgets('waits out of sight for its time, then opens at it', (
      tester,
    ) async {
      useTallView(tester);
      final createdAt = ValueNotifier<int?>(null);
      addTearDown(createdAt.dispose);
      await tester.pumpWidget(
        linked(
          _ThreadWindowRelaySession(replies),
          createdAt,
          awaitsWhileUnknown: true,
        ),
      );
      for (var frame = 0; frame < 10; frame++) {
        await tester.pump(const Duration(milliseconds: 50));
      }
      expect(findRichText('row 899'), findsOneWidget);
      expect(listVisible(tester), isFalse);

      createdAt.value = 1110;
      final rects = <Rect?>[];
      for (var frame = 0; frame < 60; frame++) {
        await tester.pump(const Duration(milliseconds: 16));
        if (listVisible(tester)) {
          rects.add(
            target.evaluate().isEmpty ? null : tester.getRect(target.first),
          );
        }
      }
      await tester.pumpAndSettle();

      expect(rects, isNotEmpty);
      expect(rects, everyElement(tester.getRect(target)));
      expect(findRichText('row 899'), findsNothing);
    });

    testWidgets('a time arriving after it gave up still lands there', (
      tester,
    ) async {
      useTallView(tester);
      final createdAt = ValueNotifier<int?>(null);
      addTearDown(createdAt.dispose);
      await tester.pumpWidget(
        linked(
          _ThreadWindowRelaySession(replies),
          createdAt,
          awaitsWhileUnknown: false,
        ),
      );
      await tester.pumpAndSettle();
      expect(target, findsNothing);

      createdAt.value = 1110;
      await tester.pumpAndSettle();

      expect(target, findsOneWidget);
    });
  });
}
