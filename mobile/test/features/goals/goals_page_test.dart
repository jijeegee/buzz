import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/goals/goal_links.dart';
import 'package:kuzz/features/goals/goal_provider.dart';
import 'package:kuzz/features/goals/goal_tree.dart';
import 'package:kuzz/features/goals/goals_page.dart';

import '../../helpers/widget_helpers.dart';

const _channel = 'channel-1';

/// Records edits instead of publishing them.
class _FakeGoalActions extends GoalActions {
  _FakeGoalActions(super.ref);
  final batches = <List<GoalOp>>[];

  @override
  Future<void> applyAll(String channelId, List<GoalOp> ops) async {
    batches.add(ops);
  }
}

/// A list read that counts its reads and hands out one more page.
class _FakeGoalList extends GoalListNotifier {
  _FakeGoalList(super.key, this.reads, this.pages);
  final List<GoalView> reads;
  final List<GoalRead> pages;

  @override
  Future<GoalRead> build() async {
    reads.add(key.view);
    return pages.first;
  }

  @override
  Future<void> loadMore() async {
    state = AsyncData(state.value!.followedBy(pages.last));
  }
}

GoalRead _page(List<GoalNode> rows, {Object? next}) => GoalRead(
  counts: (active: 0, inactive: 0, done: 0),
  tree: GoalTree(rows),
  next: next,
);

const _root = GoalNode(id: 'root', parent: null, title: 'Launch');
const _quiet = [
  GoalNode(id: 'root', parent: null, title: 'Launch', matched: false),
  GoalNode(id: 'plan', parent: 'root', title: 'Plan', matched: false),
  GoalNode(id: 'old', parent: 'plan', title: 'Old', inactive: true),
];

Future<({_FakeGoalActions Function() actions, List<GoalView> reads})> _pump(
  WidgetTester tester, {
  List<GoalNode> active = const [_root],
  GoalCounts counts = (active: 1, inactive: 0, done: 0),
  List<GoalRead> Function(GoalView)? pages,
}) async {
  late _FakeGoalActions actions;
  final reads = <GoalView>[];
  await tester.pumpWidget(
    WidgetHelpers.testable(
      overrides: [
        goalTreeProvider(_channel).overrideWith(
          (ref) async => GoalBoard(tree: GoalTree(active), counts: counts),
        ),
        goalActionsProvider.overrideWith(
          (ref) => actions = _FakeGoalActions(ref),
        ),
        goalLinkKindsProvider.overrideWith((ref, args) async => const {}),
        for (final view in const [GoalView.inactive, GoalView.done])
          goalListProvider((channelId: _channel, view: view)).overrideWith(
            () => _FakeGoalList(
              (channelId: _channel, view: view),
              reads,
              pages?.call(view) ?? const [],
            ),
          ),
      ],
      child: const GoalsPage(channelId: _channel, channelName: 'room'),
    ),
  );
  await tester.pumpAndSettle();
  return (actions: () => actions, reads: reads);
}

void main() {
  testWidgets('more than seven goals under layer 1 get a notice, no limit', (
    tester,
  ) async {
    await _pump(
      tester,
      active: const [
        GoalNode(id: 'root', parent: null, title: 'Launch', openChildren: 8),
      ],
    );
    expect(find.text('8 top-level goals'), findsOneWidget);
    expect(find.byKey(const ValueKey('goals-add-layer2')), findsOneWidget);
  });

  testWidgets('seven goals under layer 1 get no notice', (tester) async {
    await _pump(
      tester,
      active: const [
        GoalNode(id: 'root', parent: null, title: 'Launch', openChildren: 7),
      ],
    );
    expect(find.byKey(const ValueKey('goals-top-notice')), findsNothing);
  });

  testWidgets('plan goals start open, thread goals folded below them', (
    tester,
  ) async {
    await _pump(
      tester,
      active: const [
        _root,
        GoalNode(
          id: 'thread',
          parent: 'root',
          title: 'Thread',
          isThread: true,
          order: 1,
        ),
        GoalNode(id: 'task', parent: 'thread', title: 'Task', order: 2),
        GoalNode(id: 'plan', parent: 'root', title: 'Plan', order: 3),
        GoalNode(id: 'step', parent: 'plan', title: 'Step', order: 4),
      ],
    );
    expect(find.byKey(const ValueKey('goal-row-step')), findsOneWidget);
    expect(find.byKey(const ValueKey('goal-row-task')), findsNothing);
    expect(
      tester.getTopLeft(find.byKey(const ValueKey('goal-row-plan'))).dy,
      lessThan(
        tester.getTopLeft(find.byKey(const ValueKey('goal-row-thread'))).dy,
      ),
    );

    await tester.tap(find.byKey(const ValueKey('goal-fold-thread')));
    await tester.pump();
    expect(find.byKey(const ValueKey('goal-row-task')), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('goal-fold-plan')));
    await tester.pump();
    expect(find.byKey(const ValueKey('goal-row-step')), findsNothing);
  });

  testWidgets('a quiet list reads when opened, shows the path, and drops as '
      'one edit', (tester) async {
    final page = await _pump(
      tester,
      counts: (active: 1, inactive: 1, done: 0),
      pages: (_) => [_page(_quiet)],
    );
    expect(find.byKey(const ValueKey('goals-section-done')), findsNothing);
    expect(page.reads, isEmpty);

    await tester.tap(
      find.byKey(const ValueKey('goals-section-toggle-inactive')),
    );
    await tester.pumpAndSettle();
    expect(page.reads, [GoalView.inactive]);
    expect(find.byKey(const ValueKey('goal-row-old')), findsOneWidget);
    expect(find.byKey(const ValueKey('goal-row-plan')), findsNothing);
    expect(find.text('Plan'), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('goals-drop-quiet')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('goals-drop-quiet-confirm')));
    await tester.pumpAndSettle();
    expect(page.actions().batches.single.single, isA<DropQuietGoals>());
  });

  testWidgets('show more adds the next page of a list', (tester) async {
    await _pump(
      tester,
      counts: (active: 1, inactive: 0, done: 2),
      pages: (_) => [
        _page(const [
          GoalNode(id: 'root', parent: null, title: 'Launch', matched: false),
          GoalNode(id: 'one', parent: 'root', title: 'One'),
        ], next: 'cursor'),
        _page(const [
          GoalNode(id: 'root', parent: null, title: 'Launch', matched: false),
          GoalNode(id: 'two', parent: 'root', title: 'Two', order: 1),
        ]),
      ],
    );
    await tester.tap(find.byKey(const ValueKey('goals-section-toggle-done')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('goal-row-two')), findsNothing);
    await tester.tap(find.byKey(const ValueKey('goals-show-more')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('goal-row-one')), findsOneWidget);
    expect(find.byKey(const ValueKey('goal-row-two')), findsOneWidget);
    expect(find.byKey(const ValueKey('goals-show-more')), findsNothing);
  });

  testWidgets('removing sends the whole branch', (tester) async {
    tester.view.physicalSize = const Size(800, 1200);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final page = await _pump(
      tester,
      active: const [
        _root,
        GoalNode(id: 'a', parent: 'root', title: 'A'),
      ],
    );
    await tester.tap(find.byKey(const ValueKey('goal-row-a')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove'));
    await tester.pumpAndSettle();
    expect(
      find.text('Remove this goal and everything under it?'),
      findsOneWidget,
    );
    await tester.tap(find.text('Remove').last);
    await tester.pumpAndSettle();
    final remove = page.actions().batches.single.single as RemoveGoal;
    expect((remove.id, remove.recursive), ('a', true));
  });
}
