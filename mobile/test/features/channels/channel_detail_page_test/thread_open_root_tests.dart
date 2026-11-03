part of '../channel_detail_page_test.dart';

/// A relay that never answers a thread's queries, like one on a dead socket.
class _SilentRelaySession extends _IdentityUpdateRelaySession {
  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) => Completer<List<NostrEvent>>().future;

  void drop() => state = const SessionState(status: SessionStatus.reconnecting);

  void comeBack() =>
      state = const SessionState(status: SessionStatus.connected);
}

/// Serves the thread window and drops/reconnects on demand.
class _ReconnectingWindowSession extends _ThreadWindowRelaySession {
  _ReconnectingWindowSession(super.replies);

  void drop() => state = const SessionState(status: SessionStatus.reconnecting);

  void comeBack() =>
      state = const SessionState(status: SessionStatus.connected);
}

/// Records the reply tags a thread send asks for.
class _RecordingSend implements SendMessage {
  final calls = <({String? parentEventId, String? rootEventId})>[];

  @override
  Future<void> call({
    required String channelId,
    required String content,
    String? parentEventId,
    String? rootEventId,
    List<String>? mentionPubkeys,
    Channel? channel,
    List<List<String>> mediaTags = const [],
    dynamic quote,
    List<List<String>> extraTags = const [],
    bool keepFailedMessage = false,
  }) async {
    calls.add((parentEventId: parentEventId, rootEventId: rootEventId));
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

/// Reports one own reply still awaiting relay proof.
class _UnconfirmedReplyMessagesNotifier extends _FakeMessagesNotifier {
  _UnconfirmedReplyMessagesNotifier() : super(const []);

  int asked = 0;

  @override
  Set<String> unconfirmedThreadReplyIds(String rootId) {
    asked += 1;
    return {'own-pending-reply'};
  }
}

void threadOpenRootTests() {
  group('Opening a thread from its root id', () {
    final replies = [for (var n = 100; n < 110; n++) _openAtReply(n)];

    Override rootSources({
      Future<({NostrEvent event, bool deleted})?> Function()? stored,
      required Future<List<NostrEvent>> Function() relay,
    }) => threadRootSourcesProvider.overrideWithValue(
      ThreadRootSources(
        stored: (_, _) => stored?.call() ?? Future.value(),
        relay: (_, _) => relay(),
      ),
    );

    Override storedReplies(List<NostrEvent> events) =>
        paintableThreadHeadProvider.overrideWithValue(
          (_, _) async => serveThreadWindow(
            _channelId,
            _openAtRootId,
            events,
            NostrFilter(kinds: const [], limit: 50),
          ),
        );

    Widget byId(
      RelaySessionNotifier session, {
      required List<Override> overrides,
      RelayInfo? relayInfo,
      _FakeMessagesNotifier? messagesNotifier,
      _SynchronousReadStateNotifier? readState,
      Channel? standIn,
      Future<ChannelDetails>? channelDetails,
      List<Channel>? listed,
      int? firstUnread,
    }) => _buildTestable(
      messages: const [],
      messagesNotifier: messagesNotifier,
      relaySessionNotifier: session,
      huddleCurrentPubkey: 'self',
      relayInfo: relayInfo,
      channelsNotifier: _FakeChannelsNotifier(
        listed ?? [_testChannel],
        observedUnread: {
          if (firstUnread != null)
            _channelId: [
              makeObservedUnreadEvent(
                id: fixtureId(firstUnread),
                createdAt: 1000 + firstUnread,
                rootId: _openAtRootId,
                highPriority: false,
                channelType: 'stream',
                isThreadedReply: true,
              ),
            ],
        },
      ),
      readStateNotifier:
          readState ??
          _SynchronousReadStateNotifier(
            ReadStateState(
              isReady: true,
              pubkey: 'self',
              contexts: {
                _channelId: firstUnread == null ? 5000 : 999 + firstUnread,
              },
              version: 0,
            ),
          ),
      channelDetails: channelDetails,
      overrides: overrides,
      home: ThreadDetailPage(
        headId: _openAtRootId,
        initialThreadName: 'Launch plans',
        standInChannel: standIn,
        channelId: _channelId,
        currentPubkey: 'self',
        isMember: true,
        isArchived: false,
        channelBelow: false,
      ),
    );

    bool listVisible(WidgetTester tester) {
      final reveal = find.byKey(const ValueKey('thread-open-reveal'));
      if (reveal.evaluate().isEmpty) return false;
      return tester.widget<Opacity>(reveal).opacity > 0;
    }

    testWidgets('shows stored replies at once while the relay never answers', (
      tester,
    ) async {
      await tester.pumpWidget(
        byId(
          _SilentRelaySession(),
          overrides: [
            rootSources(relay: () => Completer<List<NostrEvent>>().future),
            storedReplies(replies.take(3).toList()),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();

      expect(find.byType(ThreadDetailPage), findsOneWidget);
      expect(find.text('Launch plans'), findsWidgets);
      expect(findRichText('row 102'), findsOneWidget);

      await tester.pump(const Duration(seconds: 10));
      expect(find.byType(ThreadDetailPage), findsOneWidget);
      expect(find.byType(ChannelDetailPage), findsNothing);
      expect(findRichText('row 102'), findsOneWidget);
    });

    testWidgets(
      'a relay timeout keeps the thread; a reconnect finds the root',
      (tester) async {
        var asked = 0;
        final session = _ReconnectingWindowSession(replies);
        await tester.pumpWidget(
          byId(
            session,
            overrides: [
              rootSources(
                relay: () async {
                  asked += 1;
                  if (asked == 1) throw TimeoutException('root');
                  return [_openAtRoot];
                },
              ),
            ],
          ),
        );
        await tester.pumpAndSettle();

        expect(asked, 1);
        expect(find.byType(ThreadDetailPage), findsOneWidget);
        expect(find.byType(ChannelDetailPage), findsNothing);
        expect(findRichText('row 109'), findsOneWidget);
        expect(
          find.byKey(const ValueKey('thread-root-pending')),
          findsOneWidget,
        );
        expect(
          find.byKey(const ValueKey('thread-message-deleted')),
          findsNothing,
        );

        session.drop();
        await tester.pump();
        session.comeBack();
        await tester.pumpAndSettle();

        expect(asked, 2);
        expect(find.byKey(const ValueKey('thread-root-pending')), findsNothing);
        expect(findRichText('thread root'), findsOneWidget);
      },
    );

    // A reader who joined after the root (history since joining) gets the
    // replies but never the root.
    testWidgets('an empty answer says earlier messages are not available, '
        'never deleted', (tester) async {
      await tester.pumpWidget(
        byId(
          _ThreadWindowRelaySession(replies),
          overrides: [rootSources(relay: () async => const [])],
        ),
      );
      await tester.pumpAndSettle();

      expect(find.byType(ThreadDetailPage), findsOneWidget);
      expect(find.byType(ChannelDetailPage), findsNothing);
      expect(find.byKey(const ValueKey('thread-root-pending')), findsNothing);
      expect(
        find.byKey(const ValueKey('thread-root-unavailable')),
        findsOneWidget,
      );
      expect(find.text("Earlier messages aren't available"), findsOneWidget);
      expect(
        find.byKey(const ValueKey('thread-message-deleted')),
        findsNothing,
      );
      expect(findRichText('row 109'), findsOneWidget);
    });

    testWidgets('an empty answer without replies keeps the name row', (
      tester,
    ) async {
      await tester.pumpWidget(
        byId(
          _ThreadWindowRelaySession(const []),
          overrides: [rootSources(relay: () async => const [])],
        ),
      );
      await tester.pumpAndSettle();

      expect(find.byKey(const ValueKey('thread-root-pending')), findsOneWidget);
      expect(
        find.byKey(const ValueKey('thread-root-unavailable')),
        findsNothing,
      );
    });

    testWidgets('a reply to a hidden reply hangs off an unavailable root', (
      tester,
    ) async {
      final nested = fixtureRow(
        _channelId,
        200,
        1200,
        extraTags: [
          ['e', _openAtRootId, '', 'root'],
          ['e', fixtureId(199), '', 'reply'],
        ],
      );
      await tester.pumpWidget(
        byId(
          _ThreadWindowRelaySession([nested]),
          overrides: [rootSources(relay: () async => const [])],
        ),
      );
      await tester.pumpAndSettle();

      expect(
        find.byKey(const ValueKey('thread-root-unavailable')),
        findsOneWidget,
      );
      expect(findRichText('row 200'), findsOneWidget);
    });

    testWidgets('a link at a reply under a hidden reply opens at the root', (
      tester,
    ) async {
      final target = _textMsg(
        id: 'target',
        pubkey: 'carol',
        content: 'Mention under a hidden reply',
        createdAt: 1200,
        extraTags: [
          ['e', _openAtRootId, '', 'root'],
          const ['e', 'hidden-parent', '', 'reply'],
        ],
      );
      await tester.pumpWidget(
        _buildTestable(
          messages: [target],
          messagesNotifier: _PendingHeadMessagesNotifier([
            target,
          ], Future<void>.value()),
          initialMessageId: 'target',
          initialThreadRootId: 'hidden-parent',
          initialThreadRouteBehavior:
              InitialThreadRouteBehavior.replaceCurrentRoute,
          threadReplies: {
            _openAtRootId: [target],
          },
          overrides: [rootSources(relay: () async => const [])],
        ),
      );
      await tester.pumpAndSettle();

      final page = tester.widget<ThreadDetailPage>(
        find.byType(ThreadDetailPage),
      );
      expect(page.headId, _openAtRootId);
      expect(page.initialMessageId, 'target');
      expect(
        find.byKey(const ValueKey('thread-root-unavailable')),
        findsOneWidget,
      );
      expect(find.byKey(const ValueKey('thread-message-target')), findsOne);
    });

    testWidgets('a reply under an unavailable root goes to the root id', (
      tester,
    ) async {
      final send = _RecordingSend();
      await tester.pumpWidget(
        byId(
          _ThreadWindowRelaySession(replies),
          overrides: [
            rootSources(relay: () async => const []),
            sendMessageProvider.overrideWithValue(send),
          ],
        ),
      );
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('thread-root-unavailable')),
        findsOneWidget,
      );

      final composer = tester.widget<ComposeBar>(find.byType(ComposeBar));
      expect(composer.threadHeadId, _openAtRootId);
      expect(composer.rootId, _openAtRootId);
      await composer.onSend('hello', const []);

      expect(send.calls, [
        (parentEventId: _openAtRootId, rootEventId: _openAtRootId),
      ]);
    });

    testWidgets('a failed lookup while connected is retried shortly', (
      tester,
    ) async {
      var asked = 0;
      await tester.pumpWidget(
        byId(
          _ThreadWindowRelaySession(replies),
          overrides: [
            rootSources(
              relay: () async {
                asked += 1;
                if (asked < 3) throw TimeoutException('root');
                return [_openAtRoot];
              },
            ),
          ],
        ),
      );
      await tester.pumpAndSettle();
      expect(asked, 1);

      await tester.pump(const Duration(seconds: 2));
      await tester.pumpAndSettle();
      expect(asked, 2);
      expect(find.byKey(const ValueKey('thread-root-pending')), findsOneWidget);

      await tester.pump(const Duration(seconds: 5));
      await tester.pumpAndSettle();
      expect(asked, 3);
      expect(find.byKey(const ValueKey('thread-root-pending')), findsNothing);
      expect(findRichText('thread root'), findsOneWidget);
    });

    testWidgets('only an explicit deletion shows the root as deleted', (
      tester,
    ) async {
      final removal = NostrEvent(
        id: fixtureId(9),
        pubkey: 'relay',
        createdAt: 950,
        kind: EventKind.nip29DeleteEvent,
        tags: [
          const ['h', _channelId],
          ['e', _openAtRootId],
        ],
        content: '',
        sig: '',
      );
      await tester.pumpWidget(
        byId(
          _ThreadWindowRelaySession(replies),
          overrides: [
            rootSources(relay: () async => [removal]),
          ],
        ),
      );
      await tester.pumpAndSettle();

      expect(
        find.byKey(const ValueKey('thread-message-deleted')),
        findsOneWidget,
      );
      expect(findRichText('row 109'), findsOneWidget);
      expect(find.byType(ThreadDetailPage), findsOneWidget);
    });

    testWidgets('a pending own reply settles when opened by id', (
      tester,
    ) async {
      final messages = _UnconfirmedReplyMessagesNotifier();
      await tester.pumpWidget(
        byId(
          _ThreadWindowRelaySession(replies),
          messagesNotifier: messages,
          overrides: [
            rootSources(relay: () async => [_openAtRoot]),
          ],
        ),
      );
      await tester.pumpAndSettle();

      // The thread query saw the room's unconfirmed sends: the room existed
      // before the replies were asked for.
      expect(messages.asked, greaterThan(0));
    });

    testWidgets('open at unread shows stored rows within about a second', (
      tester,
    ) async {
      await tester.pumpWidget(
        byId(
          _SilentRelaySession(),
          firstUnread: 105,
          overrides: [
            rootSources(relay: () => Completer<List<NostrEvent>>().future),
            storedReplies(replies),
            relayInfoProvider.overrideWith(
              (ref) => Completer<RelayInfo>().future,
            ),
          ],
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 1300));
      await tester.pump();

      expect(listVisible(tester), isTrue);
      expect(findRichText('row 109'), findsOneWidget);
    });

    testWidgets('a stand-in room waits the composer, not the replies', (
      tester,
    ) async {
      await tester.pumpWidget(
        byId(
          _SilentRelaySession(),
          standIn: _testChannel,
          listed: const [],
          channelDetails: Completer<ChannelDetails>().future,
          overrides: [
            rootSources(relay: () => Completer<List<NostrEvent>>().future),
            storedReplies(replies.take(2).toList()),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();

      expect(findRichText('row 101'), findsOneWidget);
      expect(find.text('Loading'), findsOneWidget);
      expect(find.byType(TextField), findsNothing);
    });

    testWidgets('a direct thread hands off at once while reconnecting', (
      tester,
    ) async {
      final chatter = _textMsg(
        id: 'chatter',
        pubkey: 'bob',
        content: 'Channel chatter',
      );
      final session = _SilentRelaySession();
      await tester.pumpWidget(
        _buildTestable(
          messages: [chatter],
          messagesNotifier: _PendingHeadMessagesNotifier([
            chatter,
          ], Completer<void>().future),
          relaySessionNotifier: session,
          initialThreadRootId: _openAtRootId,
          initialThreadRouteBehavior:
              InitialThreadRouteBehavior.replaceCurrentRoute,
          overrides: [
            rootSources(relay: () => Completer<List<NostrEvent>>().future),
          ],
        ),
      );
      session.drop();
      await tester.pump();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 500));

      expect(find.byType(ThreadDetailPage), findsOneWidget);
      expect(find.text('Channel chatter'), findsNothing);
    });

    testWidgets('a late read state never bounces a direct thread', (
      tester,
    ) async {
      final chatter = _textMsg(
        id: 'chatter',
        pubkey: 'bob',
        content: 'Channel chatter',
      );
      final readState = _LateReadStateNotifier(
        const ReadStateState(
          isReady: false,
          pubkey: 'self',
          contexts: {},
          version: 0,
        ),
      );
      await tester.pumpWidget(
        _buildTestable(
          messages: [chatter],
          messagesNotifier: _PendingHeadMessagesNotifier([
            chatter,
          ], Future<void>.error(TimeoutException('head'))..ignore()),
          readStateNotifier: readState,
          initialThreadRootId: _openAtRootId,
          initialThreadRouteBehavior:
              InitialThreadRouteBehavior.replaceCurrentRoute,
          overrides: [
            rootSources(relay: () => Completer<List<NostrEvent>>().future),
          ],
        ),
      );
      for (var frame = 0; frame < 20; frame++) {
        if (frame == 3) {
          readState.becomeReady(
            const ReadStateState(
              isReady: true,
              pubkey: 'self',
              contexts: {_channelId: 5000},
              version: 1,
            ),
          );
        }
        await tester.pump(const Duration(milliseconds: 100));
        expect(find.text('Channel chatter'), findsNothing);
      }
      await tester.pumpAndSettle();

      expect(find.byType(ThreadDetailPage), findsOneWidget);
      expect(find.text('Channel chatter'), findsNothing);
    });
  });
}
