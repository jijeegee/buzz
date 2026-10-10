import 'package:buzz/features/goals/goal_provider.dart';
import 'package:buzz/features/goals/goal_tree.dart';
import 'package:buzz/features/goals/thread_goal_chip.dart';
import 'package:buzz/features/goals/thread_goals_page.dart';
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
  testWidgets('chip shows the linked path below layer 1 and progress', (
    tester,
  ) async {
    await _pump(
      tester,
      const ThreadGoalChip(channelId: _channel, threadRootId: _thread),
    );
    expect(find.text('Server'), findsOneWidget);
    expect(find.textContaining('Launch'), findsNothing);
    expect(find.text('1/2'), findsOneWidget);
  });

  testWidgets('chip invites linking when the thread has no goal', (
    tester,
  ) async {
    await _pump(
      tester,
      const ThreadGoalChip(channelId: _channel, threadRootId: _otherThread),
    );
    expect(find.text('Link this thread to a goal'), findsOneWidget);
  });

  testWidgets('chip hides in conversations without goals', (tester) async {
    await _pump(
      tester,
      const ThreadGoalChip(channelId: _channel, threadRootId: _thread),
      tree: GoalTree.empty,
    );
    expect(find.byKey(const ValueKey('thread-goal-chip')), findsNothing);
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
