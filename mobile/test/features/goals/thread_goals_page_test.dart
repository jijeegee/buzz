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
const _editor =
    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const _thread =
    'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const _otherThread =
    'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc';
const _message =
    'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd';
const _gone =
    'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee';

GoalTree _tree() {
  var tree = GoalTree.empty;
  for (final op in <GoalOp>[
    const SetRootGoal(id: 'root', title: 'Launch'),
    const AddGoal(id: 'srv', parent: 'root', title: 'Server'),
    const AddGoal(id: 'ui', parent: 'root', title: 'Desktop UI'),
    const AddGoal(id: 'cas', parent: 'srv', title: 'CAS writes'),
    const AddGoal(id: 'val', parent: 'srv', title: 'Validation'),
    const UpdateGoal(id: 'cas', status: GoalStatus.done),
    const LinkThread(id: 'srv', thread: _thread),
  ]) {
    tree = tree.apply(op, editor: _editor, now: 1);
  }
  return tree;
}

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
          (ref) async => GoalTreeHead(
            revision: 'head',
            createdAt: 1,
            tree: tree ?? _tree(),
          ),
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
    final tree = _tree().apply(
      const UpdateGoal(id: 'srv', note: 'Keep writes atomic'),
      editor: _editor,
      now: 2,
    );
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
    final tree = _tree().apply(
      const LinkThread(id: 'ui', thread: _otherThread),
      editor: _editor,
      now: 2,
    );
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
    var tree = _tree();
    for (final id in [_otherThread, _message, _gone]) {
      tree = tree.apply(
        LinkThread(id: 'ui', thread: id),
        editor: _editor,
        now: 2,
      );
    }
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
    expect(batch, hasLength(2), reason: 'one revision');
    final add = batch[0] as AddGoal;
    final linked = batch[1] as LinkThread;
    expect((add.parent, add.title), ('ui', 'Thread panel'));
    expect((linked.id, linked.thread), (add.id, _otherThread));
  });

  testWidgets('the goal card toggles the linked goal and relinks', (
    tester,
  ) async {
    // Thread links make rows taller; keep the relink button on screen.
    tester.view.physicalSize = const Size(800, 1200);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final actions = await _pump(
      tester,
      const ThreadGoalsPage(channelId: _channel, threadRootId: _thread),
    );
    // Linking started srv; the card's status button moves it on.
    await tester.tap(find.byKey(const ValueKey('goal-status-srv')).first);
    await tester.pumpAndSettle();
    final toggle = actions.batches.single.single as UpdateGoal;
    expect((toggle.id, toggle.status), ('srv', GoalStatus.done));

    await tester.ensureVisible(
      find.byKey(const ValueKey('thread-goal-change')),
    );
    await tester.tap(find.byKey(const ValueKey('thread-goal-change')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('thread-goal-link-ui')).last);
    await tester.pumpAndSettle();
    final link = actions.batches.last.single as LinkThread;
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

  test('unlink removes the thread from every goal', () {
    final tree = _tree().apply(
      const UnlinkThread(thread: _thread),
      editor: _editor,
      now: 2,
    );
    expect(tree.forThread(_thread), isNull);
    expect(tree.node('srv')!.threads, isEmpty);
  });
}
