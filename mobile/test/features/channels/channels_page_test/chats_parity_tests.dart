part of '../channels_page_test.dart';

void chatsParityTests(Widget Function(List<Override>) app) {
  testWidgets(
    'mixed Chats pin/unpin uses persisted scoped preferences and actual message recency',
    (tester) async {
      final semantics = tester.ensureSemantics();
      SharedPreferences.setMockInitialValues({});
      final prefs = await SharedPreferences.getInstance();
      Channel chat(
        String id, {
        String type = 'stream',
        int? time,
        bool member = true,
        String visibility = 'open',
      }) => Channel(
        id: id,
        name: id,
        channelType: type,
        visibility: visibility,
        description: '',
        createdBy: 'author',
        createdAt: DateTime(2026),
        memberCount: 2,
        isMember: member,
        lastMessageAt: time == null
            ? null
            : DateTime.fromMillisecondsSinceEpoch(time * 1000),
      );
      var rows = [
        chat('older', time: 10, visibility: 'private'),
        chat('direct', type: 'dm', member: false, time: 20),
        chat('newer', time: 30),
        chat('empty-b').copyWith(name: 'A empty'),
        chat('empty-a').copyWith(name: 'Z empty'),
        chat('unjoined', member: false),
        chat('forum', type: 'forum'),
        chat('archived', time: 90).copyWith(archivedAt: DateTime(2026)),
      ];
      var channels = _MutableChatsNotifier(rows);
      var config = _ScopedChatConfig();
      Widget render() => app([
        channelsProvider.overrideWith(() => channels),
        relayConfigProvider.overrideWith(() => config),
        relaySessionProvider.overrideWith(() => _DisconnectedChatSession()),
        savedPrefsProvider.overrideWithValue(prefs),
        communityStorageProvider.overrideWithValue(
          CommunityStorage(secure: FakeSecureStorage()),
        ),
      ]);
      await tester.pumpWidget(render());
      await tester.pumpAndSettle();
      void order(List<String> names) {
        for (var i = 1; i < names.length; i++) {
          expect(
            tester.getTopLeft(find.text(names[i - 1])).dy,
            lessThan(tester.getTopLeft(find.text(names[i])).dy),
          );
        }
      }

      expect(find.text('Chats'), findsOneWidget);
      for (final hidden in ['unjoined', 'forum', 'archived']) {
        expect(find.text(hidden), findsNothing);
      }
      order(['newer', 'direct', 'older', 'Z empty', 'A empty']);

      await tester.longPress(find.text('direct'));
      await tester.pumpAndSettle();
      expect(find.text('Pin'), findsOneWidget);
      expect(find.text('Mute channel'), findsOneWidget);
      await tester.tap(find.text('Pin'));
      await tester.pumpAndSettle();
      order(['direct', 'newer', 'older']);
      expect(find.bySemanticsLabel(RegExp('Pinned')), findsOneWidget);

      rows = rows
          .map(
            (c) => c.id == 'older'
                ? c.copyWith(
                    lastMessageAt: DateTime.fromMillisecondsSinceEpoch(40000),
                  )
                : c,
          )
          .toList();
      channels.replace(rows);
      await tester.pumpAndSettle();
      order(['direct', 'older', 'newer']);
      rows = rows
          .map((c) => c.id == 'newer' ? c.copyWith(name: 'Renamed') : c)
          .toList();
      channels.replace(rows);
      await tester.pumpAndSettle();
      order(['direct', 'older', 'Renamed']);

      // Same channel IDs in another origin/account must not inherit the pin.
      config.switchTo('https://other.example', 'account-a');
      await tester.pumpAndSettle();
      order(['older', 'Renamed', 'direct']);
      config.switchTo('https://one.example', 'account-b');
      await tester.pumpAndSettle();
      order(['older', 'Renamed', 'direct']);
      config.switchTo('wss://one.example', 'account-a');
      await tester.pumpAndSettle();
      order(['direct', 'older', 'Renamed']);

      await tester.pumpWidget(const SizedBox());
      await tester.pumpAndSettle();
      channels = _MutableChatsNotifier(rows);
      config = _ScopedChatConfig();
      await tester.pumpWidget(render());
      await tester.pumpAndSettle();
      order(['direct', 'older', 'Renamed']);
      await tester.longPress(find.text('direct'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Unpin'));
      await tester.pumpAndSettle();
      order(['older', 'Renamed', 'direct']);

      // A menu opened in one account may not mutate the next account's pins.
      await tester.longPress(find.text('direct'));
      await tester.pumpAndSettle();
      config.switchTo('https://other.example', 'account-a');
      await tester.pumpAndSettle();
      await tester.tap(find.text('Pin'));
      await tester.pumpAndSettle();
      final container = ProviderScope.containerOf(
        tester.element(find.byType(ChannelsPage)),
      );
      expect(
        container.read(channelStarsProvider).store.channels['direct']?.starred,
        isNot(true),
      );
      await tester.pumpWidget(const SizedBox());
      semantics.dispose();
    },
  );
}

class _MutableChatsNotifier extends ChannelsNotifier {
  _MutableChatsNotifier(this.rows);
  List<Channel> rows;
  @override
  Future<List<Channel>> build() async => rows;
  void replace(List<Channel> next) {
    rows = next;
    state = AsyncData(next);
  }

  @override
  Future<void> ensureDirectoryLoaded() async {}
}

class _ScopedChatConfig extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(
    baseUrl: 'https://one.example',
    tokenAuth: true,
    principalId: 'account-a',
  );
  void switchTo(String origin, String account) => state = RelayConfig(
    baseUrl: origin,
    tokenAuth: true,
    principalId: account,
  );
}

class _DisconnectedChatSession extends RelaySessionNotifier {
  @override
  SessionState build() =>
      const SessionState(status: SessionStatus.disconnected);
}
