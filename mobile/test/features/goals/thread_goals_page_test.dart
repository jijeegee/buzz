import 'package:kuzz/features/goals/goal_links.dart';
import 'package:kuzz/features/goals/goal_provider.dart';
import 'package:kuzz/features/goals/goal_tree.dart';
import 'package:kuzz/features/goals/thread_goal_chip.dart';
import 'package:kuzz/features/goals/thread_goals_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../helpers/widget_helpers.dart';

const _channel = 'channel-1';
const _thread =
    'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const _otherThread =
    'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc';
const _message =
    'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd';
const _gone =
    'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee';

GoalTree _tree({
  String srvNote = '',
  List<String> uiLinks = const [],
  bool uiThread = false,
}) => GoalTree([
  const GoalNode(
    id: 'root',
    parent: null,
    title: 'Launch',
    progress: (done: 1, total: 4),
  ),
  GoalNode(
    id: 'srv',
    parent: 'root',
    title: 'Server',
    note: srvNote,
    status: GoalStatus.inProgress,
    links: const [_thread],
    isThread: true,
    progress: (done: 1, total: 2),
    order: 1,
  ),
  GoalNode(
    id: 'ui',
    parent: 'root',
    title: 'Desktop UI',
    links: uiLinks,
    isThread: uiThread,
    order: 2,
  ),
  const GoalNode(
    id: 'cas',
    parent: 'srv',
    title: 'CAS writes',
    status: GoalStatus.done,
    order: 3,
  ),
  const GoalNode(id: 'val', parent: 'srv', title: 'Validation', order: 4),
]);

/// Records edits instead of publishing them.
class _FakeGoalActions extends GoalActions {
  _FakeGoalActions(super.ref);
  final batches = <List<GoalOp>>[];

  @override
  Future<void> applyAll(String channelId, List<GoalOp> ops) async {
    batches.add(ops);
  }
}

Future<_FakeGoalActions> _pump(
  WidgetTester tester,
  Widget child, {
  GoalTree? tree,
  Map<String, GoalLinkInfo> linkKinds = const {},
}) async {
  late _FakeGoalActions actions;
  await tester.pumpWidget(
    WidgetHelpers.testable(
      overrides: [
        goalTreeProvider(_channel).overrideWith(
          (ref) async => GoalBoard(
            tree: tree ?? _tree(),
            counts: (active: 5, inactive: 0, done: 1),
          ),
        ),
        threadGoalTreeProvider.overrideWith(
          (ref, key) async => tree ?? _tree(),
        ),
        goalActionsProvider.overrideWith(
          (ref) => actions = _FakeGoalActions(ref),
        ),
        goalLinkKindsProvider.overrideWith((ref, args) async => linkKinds),
      ],
      child: child,
    ),
  );
  await tester.pumpAndSettle();
  // Read through the scope so the override is built before assertions.
  final container = ProviderScope.containerOf(
    tester.element(find.byType(Scaffold).first),
  );
  container.read(goalActionsProvider);
  return actions;
}

void main() {
  testWidgets('app bar button opens the thread goals page', (tester) async {
    await _pump(
      tester,
      const ThreadGoalsButton(channelId: _channel, threadRootId: _thread),
    );
    expect(find.byKey(const ValueKey('thread-goals-button')), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('thread-goals-button')));
    await tester.pumpAndSettle();
    expect(find.byType(ThreadGoalsPage), findsOneWidget);
    expect(find.byKey(const ValueKey('thread-goal-card')), findsOneWidget);
  });

  testWidgets('the linked goal note opens from its note button', (
    tester,
  ) async {
    final tree = _tree(srvNote: 'Keep writes atomic');
    await _pump(
      tester,
      const ThreadGoalsPage(channelId: _channel, threadRootId: _thread),
      tree: tree,
    );
    expect(find.text('Keep writes atomic'), findsNothing);
    await tester.tap(find.byKey(const ValueKey('goal-note-toggle-srv')));
    await tester.pump();
    expect(find.text('Keep writes atomic'), findsOneWidget);
  });

  testWidgets('thread links open other threads; this thread stays put', (
    tester,
  ) async {
    final tree = _tree(uiLinks: [_otherThread]);
    await _pump(
      tester,
      const ThreadGoalsPage(channelId: _channel, threadRootId: _thread),
      tree: tree,
    );
    InkWell link(String thread) => tester.widget<InkWell>(
      find.descendant(
        of: find.byKey(ValueKey('goal-thread-$thread')),
        matching: find.byType(InkWell),
      ),
    );
    expect(find.byTooltip('This thread'), findsOneWidget);
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('goal-thread-$_thread')),
        matching: find.byType(Text),
      ),
      findsNothing,
      reason: 'links are icons only',
    );
    expect(link(_thread).onTap, isNull);
    // The other branch is read-only and faded, but its thread still opens.
    expect(link(_otherThread).onTap, isNotNull);
  });

  testWidgets('message links say where they go; missing ones are disabled', (
    tester,
  ) async {
    final tree = _tree(uiLinks: [_otherThread, _message, _gone]);
    await _pump(
      tester,
      const ThreadGoalsPage(channelId: _channel, threadRootId: _thread),
      tree: tree,
      linkKinds: {
        _otherThread: (kind: GoalLinkKind.thread, threadRootId: null),
        _message: (kind: GoalLinkKind.message, threadRootId: null),
        _gone: (kind: GoalLinkKind.missing, threadRootId: null),
      },
    );
    Finder link(String id) => find.byKey(ValueKey('goal-thread-$id'));
    InkWell ink(String id) => tester.widget<InkWell>(
      find.descendant(of: link(id), matching: find.byType(InkWell)),
    );
    String? label(String id) => tester
        .widget<Tooltip>(
          find.descendant(of: link(id), matching: find.byType(Tooltip)),
        )
        .message;
    expect(label(_otherThread), 'Open thread');
    expect(label(_message), 'Go to message');
    expect(label(_gone), 'Go to message');
    expect(ink(_message).onTap, isNotNull);
    expect(ink(_gone).onTap, isNull);
  });

  testWidgets('a message links only to the goals its place allows', (
    tester,
  ) async {
    Future<_FakeGoalActions> open(String? threadRootId) async {
      // A fresh provider scope per opening.
      await tester.pumpWidget(const SizedBox());
      final actions = await _pump(
        tester,
        Scaffold(
          body: Consumer(
            builder: (context, ref, _) => TextButton(
              key: const ValueKey('open'),
              onPressed: () => showMessageGoalLink(
                context: Navigator.of(context).context,
                channelId: _channel,
                messageId: _message,
                threadRootId: threadRootId,
              ),
              child: const Text('open'),
            ),
          ),
        ),
      );
      await tester.tap(find.byKey(const ValueKey('open')));
      await tester.pumpAndSettle();
      return actions;
    }

    InkWell? option(String id) {
      final found = find.byKey(ValueKey('thread-goal-link-$id'));
      return found.evaluate().isEmpty ? null : tester.widget<InkWell>(found);
    }

    // Main timeline: layer 2 and below; a layer 2 goal may go under layer 1.
    final main = await open(null);
    expect(option('root')!.onTap, isNull);
    expect(
      find.byKey(const ValueKey('thread-goal-create-under-root')),
      findsOneWidget,
    );
    await tester.tap(find.byKey(const ValueKey('thread-goal-link-ui')));
    await tester.pumpAndSettle();
    final link = main.batches.single.single as LinkThread;
    expect((link.id, link.thread), ('ui', _message));

    // In a linked thread: only the thread's goal and its sub-goals.
    await open(_thread);
    for (final id in ['srv', 'cas', 'val']) {
      expect(option(id)!.onTap, isNotNull, reason: id);
    }
    expect(option('root'), isNull);
    expect(option('ui'), isNull);
  });

  testWidgets('linked page shows the sub-goals and edits them', (tester) async {
    final actions = await _pump(
      tester,
      const ThreadGoalsPage(channelId: _channel, threadRootId: _thread),
    );
    expect(find.byKey(const ValueKey('thread-goal-card')), findsOneWidget);
    expect(find.byKey(const ValueKey('goal-row-cas')), findsOneWidget);
    expect(find.byKey(const ValueKey('goal-row-val')), findsOneWidget);
    expect(find.text('1 of 2 sub-goals done'), findsOneWidget);

    // The whole tree shows: the path is read-only, the rest is also faded.
    bool faded(String id) => find
        .ancestor(
          of: find.byKey(ValueKey('goal-status-$id')),
          matching: find.byType(Opacity),
        )
        .evaluate()
        .isNotEmpty;
    for (final id in ['root', 'ui']) {
      expect(find.byKey(ValueKey('goal-row-$id')), findsOneWidget);
      final status = tester.widget<IconButton>(
        find.byKey(ValueKey('goal-status-$id')),
      );
      expect(status.onPressed, isNull, reason: '$id is read-only');
    }
    expect(faded('root'), isFalse);
    expect(faded('ui'), isTrue);
    expect(faded('val'), isFalse);
    await tester.tap(find.byKey(const ValueKey('goal-row-ui')));
    await tester.pumpAndSettle();
    expect(actions.batches, isEmpty);

    await tester.tap(find.byKey(const ValueKey('goal-status-val')));
    await tester.pumpAndSettle();
    final toggle = actions.batches.single.single as UpdateGoal;
    expect(toggle.id, 'val');
    expect(toggle.status, GoalStatus.inProgress);

    await tester.tap(find.byKey(const ValueKey('thread-goal-add')));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('goal-editor-title')),
      'Retry on conflict',
    );
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('goal-editor-save')));
    await tester.pumpAndSettle();
    final add = actions.batches.last.single as AddGoal;
    expect(add.parent, 'srv');
    expect(add.title, 'Retry on conflict');

    await tester.scrollUntilVisible(
      find.byKey(const ValueKey('thread-goal-unlink')),
      200,
      scrollable: find.byType(Scrollable).first,
    );
    await tester.tap(find.byKey(const ValueKey('thread-goal-unlink')));
    await tester.pumpAndSettle();
    final unlink = actions.batches.last.single as UnlinkThread;
    expect(unlink.thread, _thread);
  });

  testWidgets('unlinked page links, or adds a goal and links it at once', (
    tester,
  ) async {
    final actions = await _pump(
      tester,
      const ThreadGoalsPage(channelId: _channel, threadRootId: _otherThread),
    );
    await tester.tap(find.byKey(const ValueKey('thread-goal-link-ui')));
    await tester.pumpAndSettle();
    final link = actions.batches.single.single as LinkThread;
    expect((link.id, link.thread), ('ui', _otherThread));

    await tester.tap(find.byKey(const ValueKey('thread-goal-create-under-ui')));
    await tester.pumpAndSettle();
    expect(find.text('New layer 3 goal'), findsOneWidget);
    await tester.enterText(
      find.byKey(const ValueKey('goal-editor-title')),
      'Thread panel',
    );
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('goal-editor-save')));
    await tester.pumpAndSettle();
    final batch = actions.batches.last;
    expect(batch, hasLength(2), reason: 'one event');
    final add = batch[0] as AddGoal;
    final linked = batch[1] as LinkThread;
    expect((add.parent, add.title), ('ui', 'Thread panel'));
    expect((linked.id, linked.thread), (add.id, _otherThread));
  });

  testWidgets('the linked goal relinks', (tester) async {
    // Thread links make rows taller; keep the relink button on screen.
    tester.view.physicalSize = const Size(800, 1200);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final actions = await _pump(
      tester,
      const ThreadGoalsPage(channelId: _channel, threadRootId: _thread),
    );
    await tester.ensureVisible(
      find.byKey(const ValueKey('thread-goal-change')),
    );
    await tester.tap(find.byKey(const ValueKey('thread-goal-change')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('thread-goal-link-ui')).last);
    await tester.pumpAndSettle();
    final link = actions.batches.single.single as LinkThread;
    expect((link.id, link.thread), ('ui', _thread));

    await tester.ensureVisible(
      find.byKey(const ValueKey('thread-goal-change')),
    );
    await tester.tap(find.byKey(const ValueKey('thread-goal-change')));
    await tester.pumpAndSettle();
    await tester.tap(
      find.byKey(const ValueKey('thread-goal-create-under-ui')).last,
    );
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('goal-editor-title')),
      'Relinked work',
    );
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('goal-editor-save')));
    await tester.pumpAndSettle();
    final batch = actions.batches.last;
    expect(batch, hasLength(2));
    expect((batch[0] as AddGoal).parent, 'ui');
    expect((batch[1] as LinkThread).thread, _thread);
  });

  testWidgets('thread goals fold, but this thread\'s path stays open', (
    tester,
  ) async {
    final tree = GoalTree([
      ..._tree(uiLinks: [_otherThread], uiThread: true).nodes,
      const GoalNode(id: 'ui-task', parent: 'ui', title: 'Icons'),
    ]);
    await _pump(
      tester,
      const ThreadGoalsPage(channelId: _channel, threadRootId: _thread),
      tree: tree,
    );
    expect(find.byKey(const ValueKey('goal-row-val')), findsOneWidget);
    expect(find.byKey(const ValueKey('goal-row-ui')), findsOneWidget);
    expect(find.byKey(const ValueKey('goal-row-ui-task')), findsNothing);
    await tester.tap(find.byKey(const ValueKey('goal-fold-ui')));
    await tester.pump();
    expect(find.byKey(const ValueKey('goal-row-ui-task')), findsOneWidget);
  });

  testWidgets('finishing a thread goal with tasks asks first', (tester) async {
    final actions = await _pump(
      tester,
      const ThreadGoalsPage(channelId: _channel, threadRootId: _thread),
    );
    await tester.tap(find.byKey(const ValueKey('goal-status-srv')).first);
    await tester.pumpAndSettle();
    expect(actions.batches, isEmpty);
    await tester.tap(find.byKey(const ValueKey('goal-finish-confirm-srv')));
    await tester.pumpAndSettle();
    final toggle = actions.batches.single.single as UpdateGoal;
    expect((toggle.id, toggle.status), ('srv', GoalStatus.done));
  });
}
