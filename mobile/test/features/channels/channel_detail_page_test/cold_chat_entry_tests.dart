part of '../channel_detail_page_test.dart';

const _coldPubkey =
    'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc';
const _coldScope = MessageScope(
  relayUrl: 'https://relay.test',
  pubkey: _coldPubkey,
);

class _ColdEntryConfig extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(
    baseUrl: 'https://relay.test',
    tokenAuth: true,
    principalId: _coldPubkey,
  );
}

/// No server query answers. A cached row must still lead to editable chat.
class _ColdEntryRelay extends _IdentityUpdateRelaySession {
  _ColdEntryRelay(this.status);
  final SessionStatus status;
  final pending = Completer<List<NostrEvent>>();
  final head = Completer<List<NostrEvent>>();
  final replies = Completer<List<NostrEvent>>();

  @override
  SessionState build() => SessionState(status: status);

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) => filter.ids?.contains(_openAtRootId) == true
      ? head.future
      : pending.future;

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) => filters.any((filter) => filter.extensions['thread_window'] == true)
      ? replies.future
      : pending.future;
}

void coldChatEntryTests() {
  for (final status in [SessionStatus.connecting, SessionStatus.connected]) {
    for (final thread in [false, true]) {
      testWidgets('cold Chats entry: $status, thread=$thread, no server answer', (
        tester,
      ) async {
        final root = Directory.systemTemp.createTempSync('cold_chat_entry');
        final store = MessageStore(() async => root.path, rootPath: root.path);
        final events = [
          for (var n = 100; n < 110; n++)
            if (thread) _openAtReply(n) else _openAtRow(n),
        ];
        final latest = events.last;
        await tester.runAsync(() async {
          await store.write(
            _coldScope,
            MessageDbWrite(
              rows: messageRowsFor(events, receivedAt: 1),
              claims: [
                CoverageClaim(
                  channelId: _channelId,
                  stream: thread
                      ? CoverageStream.thread(_openAtRootId)
                      : CoverageStream.main,
                  range: CoverageRange(
                    CoverageCursor.start,
                    CoverageCursor(latest.createdAt, latest.id),
                  ),
                ),
              ],
            ),
          );
          await store.settled();
        });
        // Only the Chats cache has the thread root, outside the room's newest
        // stored page. Real providers read both caches on this cold start.
        final row = ChatListRow(
          isThread: thread,
          key: thread ? _openAtRootId : _channelId,
          at: latest.createdAt,
          atUs: latest.createdAt * 1000000,
          channelId: _channelId,
          channelType: 'stream',
          channelName: 'general',
          rootId: thread ? _openAtRootId : null,
          root: thread ? _openAtRoot : null,
          latest: latest,
        );
        await _testPrefs.setString(
          'kuzz.chatList.v1:${storageScopeFor(_coldScope.relayUrl)}:$_coldPubkey',
          jsonEncode({
            'chat_list': 1,
            'rows': [row.toJson()],
          }),
        );
        final session = _ColdEntryRelay(status);
        await tester.pumpWidget(
          _buildTestable(
            home: const ChatsPage(),
            messages: const [],
            realMessages: true,
            realChannels: true,
            loadChannelDetails: () => Completer<ChannelDetails>().future,
            relayConfigNotifier: _ColdEntryConfig(),
            relaySessionNotifier: session,
            huddleCurrentPubkey: _coldPubkey,
            readStateNotifier: _SynchronousReadStateNotifier(
              const ReadStateState(
                isReady: true,
                pubkey: _coldPubkey,
                contexts: {},
                version: 0,
              ),
            ),
            overrides: [messageStoreProvider.overrideWithValue(store)],
          ),
        );
        await tester.pump();
        final container = ProviderScope.containerOf(
          tester.element(find.byType(ChatsPage)),
        );
        expect(container.read(chatListProvider).requireValue.fromCache, isTrue);
        expect(container.read(channelsProvider).isLoading, isTrue);
        await tester.tap(find.byKey(ValueKey('chat-row-tap-${row.key}')));
        // Bounded UI frames, with no query completion or seconds-long wait.
        for (var frame = 0; frame < 30; frame++) {
          await tester.pump(const Duration(milliseconds: 16));
        }
        expect(session.pending.isCompleted, isFalse);
        expect(container.read(channelsProvider).isLoading, isTrue);
        expect(find.text('This chat is still loading.'), findsNothing);
        expect(
          find.byType(thread ? ThreadDetailPage : ChannelDetailPage),
          findsOneWidget,
        );
        expect(find.byType(ComposeBar), findsOneWidget);
        // The normal compact composer expands only when the user taps it.
        await tester.tap(
          find.descendant(
            of: find.byType(ComposeBar),
            matching: find.byWidgetPredicate(
              (widget) =>
                  widget is Text &&
                  (widget.data?.startsWith(
                        thread ? 'Reply in thread' : 'Message',
                      ) ??
                      false),
            ),
          ),
        );
        for (var frame = 0; frame < 20; frame++) {
          await tester.pump(const Duration(milliseconds: 16));
        }
        final input = find.byType(TextField);
        await tester.enterText(input, 'ready before the server answers');
        expect(find.text('ready before the server answers'), findsOneWidget);
        expect(findRichText('row 109'), findsOneWidget);
        if (thread && status == SessionStatus.connected) {
          // Finishing the background requests updates the held snapshot.
          // The replacement thread route must still own the head request.
          session.head.complete([_openAtRoot]);
          session.replies.complete(
            serveThreadWindow(
              _channelId,
              _openAtRootId,
              [...events, _openAtReply(110)],
              threadWindowFilter(channelId: _channelId, rootId: _openAtRootId),
            ),
          );
          for (var frame = 0; frame < 20; frame++) {
            await tester.pump(const Duration(milliseconds: 16));
          }
          final notifier = container.read(
            channelMessagesProvider(_channelId).notifier,
          );
          expect(notifier.deepLinkEvent(_openAtRootId)?.id, _openAtRootId);
          final updated = container
              .read(
                threadRepliesWithLocalProvider(
                  ThreadRepliesArgs(
                    channelId: _channelId,
                    rootId: _openAtRootId,
                  ),
                ),
              )
              .requireValue;
          expect(updated.any((event) => event.id == fixtureId(110)), isTrue);
          Navigator.of(tester.element(find.byType(ThreadDetailPage))).pop();
          for (var frame = 0; frame < 30; frame++) {
            await tester.pump(const Duration(milliseconds: 16));
          }
          expect(notifier.deepLinkEvent(_openAtRootId), isNull);
        }
        await tester.pumpWidget(const SizedBox());
        await tester.runAsync(() async {
          await store.close();
          root.deleteSync(recursive: true);
        });
      });
    }
  }
}
