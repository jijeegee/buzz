import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/goals/goal_provider.dart';
import 'package:kuzz/features/goals/goal_tree.dart';

const _thread =
    'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';

Map<String, dynamic> _row(
  String id,
  String? parent, [
  Map<String, dynamic> extra = const {},
]) => {
  'id': id,
  'parent': parent,
  'title': id,
  'note': '',
  'status': 'open',
  'order': 0,
  'assignees': <String>[],
  'links': <String>[],
  'updated_by': null,
  'updated_at': 0,
  'matched': true,
  'inactive': false,
  'is_thread': false,
  'progress': {'done': 0, 'total': 0},
  'open_children': 0,
  ...extra,
};

GoalTree _rows(List<Map<String, dynamic>> rows) =>
    GoalTree([for (final row in rows) GoalNode.fromJson(row)]);

void main() {
  test('reads the relay row shape', () {
    final read = GoalRead.fromJson({
      'goals': 1,
      'counts': {'active': 2, 'inactive': 1, 'done': 3},
      'rows': [
        _row('root', null),
        _row('a', 'root', {
          'note': 'Key info',
          'status': 'in_progress',
          'links': [_thread],
          'updated_by': 'editor',
          'updated_at': 9,
          'matched': false,
          'inactive': true,
          'is_thread': true,
          'progress': {'done': 1, 'total': 2},
          'open_children': 1,
        }),
      ],
      'threads': <Object>[],
      'next': {'order': 3, 'id': 'a'},
    });
    expect(read.counts, (active: 2, inactive: 1, done: 3));
    expect(read.next, {'order': 3, 'id': 'a'});
    final a = read.tree.node('a')!;
    expect(a.note, 'Key info');
    expect(a.status, GoalStatus.inProgress);
    expect(a.links, [_thread]);
    expect(a.updatedBy, 'editor');
    expect(a.updatedAt, 9);
    expect(a.matched, isFalse);
    expect(a.inactive, isTrue);
    expect(a.isThread, isTrue);
    expect(a.progress, (done: 1, total: 2));
    expect(a.openChildren, 1);
    expect(read.tree.forThread(_thread.toUpperCase())?.id, 'a');
  });

  test('sends each edit the way kuzz_core reads it', () {
    expect(
      [
        const SetRootGoal(id: 'r', title: 'Ship', note: 'Key'),
        const AddGoal(id: 'a', parent: 'r', title: 'A'),
        const UpdateGoal(id: 'a', status: GoalStatus.inProgress),
        const UpdateGoal(id: 'a', title: 'B', note: ''),
        const RemoveGoal(id: 'a', recursive: true),
        LinkThread(id: 'a', thread: _thread.toUpperCase()),
        UnlinkThread(thread: _thread.toUpperCase()),
        const DropQuietGoals(),
      ].map((op) => jsonDecode(jsonEncode(op.toJson()))),
      [
        {'op': 'set_root', 'id': 'r', 'title': 'Ship', 'note': 'Key'},
        {'op': 'add', 'id': 'a', 'parent': 'r', 'title': 'A'},
        {'op': 'update', 'id': 'a', 'status': 'in_progress'},
        {'op': 'update', 'id': 'a', 'title': 'B', 'note': ''},
        {'op': 'remove', 'id': 'a', 'recursive': true},
        {'op': 'link', 'id': 'a', 'thread': _thread},
        {'op': 'unlink', 'thread': _thread},
        {'op': 'drop_inactive'},
      ],
    );
  });

  test(
    'outline is depth-first with layers; plan goals before thread goals',
    () {
      final tree = _rows([
        _row('root', null),
        _row('t', 'root', {'is_thread': true, 'order': 1}),
        _row('a', 'root', {'order': 2}),
        _row('a1', 'a', {'order': 3}),
      ]);
      expect(tree.outline().map((row) => '${row.layer}:${row.node.id}'), [
        '1:root',
        '2:a',
        '3:a1',
        '2:t',
      ]);
    },
  );

  test('a plan goal has no thread link here or above', () {
    final tree = _rows([
      _row('root', null),
      _row('t', 'root', {'is_thread': true}),
      _row('task', 't'),
      _row('plan', 'root'),
    ]);
    expect(tree.isPlanGoal('plan'), isTrue);
    expect(tree.isPlanGoal('t'), isFalse);
    expect(tree.isPlanGoal('task'), isFalse);
  });

  test('a later page does not unselect a goal', () {
    final merged = mergeGoalTrees([
      _rows([_row('root', null), _row('a', 'root')]),
      _rows([
        _row('root', null, {'matched': false}),
        _row('a', 'root', {'matched': false}),
        _row('b', 'a'),
      ]),
    ]);
    expect(merged.nodes.map((node) => (node.id, node.matched)), [
      ('root', true),
      ('a', true),
      ('b', true),
    ]);
    expect(
      merged.listTops().map((node) => node.id),
      ['a'],
      reason: 'layer 1 stays out of a list and hides nothing below it',
    );
  });

  test('a list shows selected goals at the top, in order', () {
    final tree = _rows([
      _row('root', null, {'matched': false}),
      _row('plan', 'root', {'matched': false}),
      _row('old', 'plan', {'order': 2}),
      _row('older', 'plan', {'order': 1}),
      _row('below', 'old', {'order': 3}),
    ]);
    expect(tree.listTops().map((node) => node.id), ['older', 'old']);
  });

  test('finishing warns only when it deletes tasks', () {
    final tree = _rows([
      _row('t', null, {
        'is_thread': true,
        'status': 'in_progress',
        'progress': {'done': 0, 'total': 2},
        'open_children': 2,
      }),
      _row('plan', 't', {
        'progress': {'done': 0, 'total': 2},
      }),
      _row('bare', 't', {'is_thread': true}),
      _row('over', 't', {
        'is_thread': true,
        'status': 'done',
        'open_children': 1,
      }),
    ]);
    expect(tree.node('t')!.finishDeletesTasks(GoalStatus.done), isTrue);
    expect(tree.node('t')!.finishDeletesTasks(GoalStatus.dropped), isTrue);
    expect(tree.node('t')!.finishDeletesTasks(GoalStatus.open), isFalse);
    expect(tree.node('plan')!.finishDeletesTasks(GoalStatus.done), isFalse);
    expect(tree.node('bare')!.finishDeletesTasks(GoalStatus.done), isFalse);
    expect(tree.node('over')!.finishDeletesTasks(GoalStatus.dropped), isFalse);
  });

  test('reads go by the relay filter shape', () {
    expect(goalViewFilter('c', GoalView.inactive, cursor: {'id': 'a'}), {
      'goals': 1,
      'channel': 'c',
      'view': 'inactive',
      'cursor': {'id': 'a'},
    });
    expect(goalViewFilter('c', GoalView.active), {
      'goals': 1,
      'channel': 'c',
      'view': 'active',
    });
    expect(goalThreadsFilter('c', [_thread]), {
      'goals': 1,
      'channel': 'c',
      'threads': [_thread],
    });
    expect(threadGoalsKey('c', ['AA', 'bb']), (
      channelId: 'c',
      threads: 'aa,bb',
    ));
  });
}
