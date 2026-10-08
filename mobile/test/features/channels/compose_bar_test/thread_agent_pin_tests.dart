part of '../compose_bar_test.dart';

void threadAgentPinTests() {
  group('thread automatic agent mentions', () {
    final owner = 'd' * 64;
    final agent = 'e' * 64;
    // A stable signer keeps the draft store's identity across remounts.
    final signer = nostr.Keys.generate();

    Widget build({
      String? thread = 'thread-head',
      List<List<String>> rootTags = const [],
      void Function(List<String>)? onSent,
    }) => _buildComposeBar(
      threadHeadId: thread,
      threadRootTags: rootTags,
      currentPubkey: owner,
      uploadService: _testUploadService(signer.nsec),
      relayConfig: () => _SwitchableRelayConfigNotifier(
        RelayConfig(baseUrl: 'http://localhost:3000', nsec: signer.nsec),
      ),
      relayAgents: [_testAgent(agent)],
      channels: [_makeCurrentChannel()],
      members: [
        ChannelMember(
          pubkey: agent,
          displayName: 'Helper Bot',
          role: 'bot',
          joinedAt: DateTime(2025),
        ),
      ],
      onSend: (_, mentions, {mediaTags = const []}) async {
        onSent?.call(mentions);
      },
    );

    String draft(WidgetTester tester) =>
        tester.widget<TextField>(find.byType(TextField)).controller!.text;

    ProviderContainer container(WidgetTester tester) =>
        ProviderScope.containerOf(tester.element(find.byType(ComposeBar)));

    Future<void> enable(WidgetTester tester) => container(
      tester,
    ).read(keepMentionedAgentsPinnedProvider.notifier).setEnabled(true);

    final chips = find.byKey(const ValueKey('composer-address-locks'));

    testWidgets('is off by default: root agents are not added', (tester) async {
      await tester.pumpWidget(
        build(
          rootTags: [
            ['p', agent],
          ],
        ),
      );
      await _expandComposer(tester);
      await tester.pumpAndSettle();
      expect(draft(tester), isEmpty);
      expect(chips, findsNothing);
    });

    testWidgets('adds root agents and keeps them after each reply', (
      tester,
    ) async {
      final sent = <List<String>>[];
      await tester.pumpWidget(
        build(
          rootTags: [
            ['p', agent],
            ['p', owner],
          ],
          onSent: sent.add,
        ),
      );
      await enable(tester);
      await _expandComposer(tester);
      await tester.pumpAndSettle();
      expect(draft(tester), '@Helper Bot ');
      expect(
        find.byKey(ValueKey('composer-address-lock-$agent')),
        findsOneWidget,
      );

      await tester.enterText(find.byType(TextField), '@Helper Bot hello');
      await tester.tap(find.byIcon(LucideIcons.arrowUp));
      await tester.pumpAndSettle();
      expect(sent, [
        [agent],
      ]);
      expect(draft(tester), '@Helper Bot ');
      // The automatic prefix alone is not an authored draft.
      final drafts = container(tester).read(composeDraftsProvider.notifier);
      final key = composeDraftKey('channel-1', threadHeadId: 'thread-head');
      expect(drafts.draftFor(key), isNull);
      await tester.enterText(find.byType(TextField), '@Helper Bot later');
      await tester.pumpAndSettle();
      expect(drafts.draftFor(key)?.text, 'later');
      // Let the draft store's debounced write finish.
      await tester.pump(const Duration(seconds: 5));
    });

    testWidgets('deleting the mention or the chip stops auto-mentioning', (
      tester,
    ) async {
      await tester.pumpWidget(
        build(
          rootTags: [
            ['p', agent],
          ],
        ),
      );
      await enable(tester);
      await _expandComposer(tester);
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), 'hello');
      await tester.pumpAndSettle();
      expect(chips, findsNothing);
      await tester.tap(find.byIcon(LucideIcons.arrowUp));
      await tester.pumpAndSettle();
      expect(draft(tester), isEmpty);

      // Re-pin through the picker, then remove it with the chip.
      await tester.enterText(find.byType(TextField), 'hi @hel');
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(ValueKey('mention-always-address-$agent')));
      await tester.pumpAndSettle();
      expect(draft(tester), '@Helper Bot hi ');
      await tester.tap(find.byKey(ValueKey('composer-address-lock-$agent')));
      await tester.pumpAndSettle();
      expect(draft(tester), 'hi ');
      expect(chips, findsNothing);
    });

    testWidgets('deleting one automatic mention saves no phantom draft', (
      tester,
    ) async {
      final second = 'f' * 64;
      await tester.pumpWidget(
        _buildComposeBar(
          threadHeadId: 'thread-head',
          threadRootTags: [
            ['p', agent],
            ['p', second],
          ],
          currentPubkey: owner,
          uploadService: _testUploadService(nostr.Keys.generate().nsec),
          relayAgents: [
            _testAgent(agent),
            AgentDirectoryEntry(pubkey: second, displayName: 'Scout'),
          ],
          channels: [_makeCurrentChannel()],
          members: [
            ChannelMember(
              pubkey: agent,
              displayName: 'Helper Bot',
              role: 'bot',
              joinedAt: DateTime(2025),
            ),
            ChannelMember(
              pubkey: second,
              displayName: 'Scout',
              role: 'bot',
              joinedAt: DateTime(2025),
            ),
          ],
          onSend: (_, _, {mediaTags = const []}) async {},
        ),
      );
      await enable(tester);
      await _expandComposer(tester);
      await tester.pumpAndSettle();
      expect(draft(tester), '@Helper Bot @Scout ');
      await tester.enterText(find.byType(TextField), '@Scout ');
      await tester.pumpAndSettle();
      final drafts = container(tester).read(composeDraftsProvider.notifier);
      final key = composeDraftKey('channel-1', threadHeadId: 'thread-head');
      expect(drafts.draftFor(key), isNull);
      await tester.enterText(find.byType(TextField), '@Scout hello');
      await tester.pumpAndSettle();
      expect(drafts.draftFor(key)?.text, 'hello');
      expect(
        find.byKey(ValueKey('composer-address-lock-$agent')),
        findsNothing,
      );
      await tester.pump(const Duration(seconds: 5));
    });

    testWidgets('mentioning an agent pins it and offers Turn off', (
      tester,
    ) async {
      await tester.pumpWidget(build());
      await enable(tester);
      await _expandComposer(tester);
      await tester.enterText(find.byType(TextField), '@hel');
      await tester.pumpAndSettle();
      await tester.tap(find.text('Helper Bot'));
      await tester.pumpAndSettle();
      expect(
        find.text('Helper Bot will be mentioned automatically'),
        findsOneWidget,
      );
      expect(
        find.byKey(ValueKey('composer-address-lock-$agent')),
        findsOneWidget,
      );
      await tester.tap(find.text('Turn off'));
      await tester.pumpAndSettle();
      expect(container(tester).read(keepMentionedAgentsPinnedProvider), false);
      expect(chips, findsNothing);
    });

    testWidgets('confirmation sits above the input and clears after 3s', (
      tester,
    ) async {
      await tester.pumpWidget(build());
      await enable(tester);
      await _expandComposer(tester);
      await tester.enterText(find.byType(TextField), '@hel');
      await tester.pumpAndSettle();
      await tester.tap(find.text('Helper Bot'));
      await tester.pump();
      final notice = find.byKey(
        const ValueKey('composer-auto-pin-confirmation'),
      );
      expect(notice, findsOneWidget);
      expect(find.byType(SnackBar), findsNothing);
      expect(
        tester.getRect(notice).bottom,
        lessThanOrEqualTo(tester.getRect(find.byType(TextField)).top),
      );
      await tester.pump(const Duration(seconds: 3));
      await tester.pumpAndSettle();
      expect(notice, findsNothing);
      expect(container(tester).read(keepMentionedAgentsPinnedProvider), true);
    });

    testWidgets('a restored draft shows its agent mention as a chip', (
      tester,
    ) async {
      final agentChip = find.byWidgetPredicate(
        (widget) =>
            widget.runtimeType.toString() == '_ComposerAgentMentionChip',
      );
      await tester.pumpWidget(build());
      await _expandComposer(tester);
      await tester.enterText(find.byType(TextField), '@hel');
      await tester.pumpAndSettle();
      await tester.tap(find.text('Helper Bot'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), '@Helper Bot hello');
      await tester.pumpAndSettle();
      expect(agentChip, findsOneWidget);

      // Leave the thread and come back: the draft reloads from storage.
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pump(const Duration(seconds: 5));
      await tester.pumpWidget(build());
      await tester.pumpAndSettle();
      await tester.tap(find.text('@Helper Bot hello'));
      await tester.pumpAndSettle();
      expect(draft(tester), '@Helper Bot hello');
      expect(agentChip, findsOneWidget);
      await tester.pump(const Duration(seconds: 5));
    });

    testWidgets('picker pin turns the preference on', (tester) async {
      await tester.pumpWidget(build());
      await _expandComposer(tester);
      await tester.enterText(find.byType(TextField), '@hel');
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('mention-keep-agents-pinned-toggle')),
        findsOneWidget,
      );
      await tester.tap(find.byKey(ValueKey('mention-always-address-$agent')));
      await tester.pumpAndSettle();
      expect(container(tester).read(keepMentionedAgentsPinnedProvider), true);
      expect(draft(tester), '@Helper Bot ');
    });

    final popover = find.byKey(const ValueKey('mention-suggestions-popover'));

    testWidgets('an automatic mention prefix does not open the picker', (
      tester,
    ) async {
      await tester.pumpWidget(
        build(
          rootTags: [
            ['p', agent],
          ],
        ),
      );
      await enable(tester);
      await _expandComposer(tester);
      await tester.pumpAndSettle();
      expect(draft(tester), '@Helper Bot ');
      // Tapping back into the field puts the cursor after the prefix.
      final field = tester.widget<TextField>(find.byType(TextField));
      field.controller!.selection = const TextSelection.collapsed(offset: 12);
      await tester.pumpAndSettle();
      expect(popover, findsNothing);
      await tester.enterText(find.byType(TextField), '@Helper Bot @hel');
      await tester.pumpAndSettle();
      expect(
        popover,
        findsOneWidget,
      ); // Let the draft store's debounced write finish.
      await tester.pump(const Duration(seconds: 5));
    });

    testWidgets('closing the composer dismisses the mention picker', (
      tester,
    ) async {
      await tester.pumpWidget(build());
      await _expandComposer(tester);
      await tester.enterText(find.byType(TextField), '@hel');
      await tester.pumpAndSettle();
      expect(popover, findsOneWidget);
      FocusManager.instance.primaryFocus?.unfocus();
      await tester.pumpAndSettle();
      expect(popover, findsNothing);
    });

    testWidgets('channel composers have no automatic mentions', (tester) async {
      await tester.pumpWidget(build(thread: null));
      await enable(tester);
      await _expandComposer(tester);
      await tester.enterText(find.byType(TextField), '@hel');
      await tester.pumpAndSettle();
      expect(find.text('Helper Bot'), findsOneWidget);
      expect(
        find.byKey(ValueKey('mention-always-address-$agent')),
        findsNothing,
      );
      await tester.tap(find.text('Helper Bot'));
      await tester.pumpAndSettle();
      expect(chips, findsNothing);
    });
  });
}
