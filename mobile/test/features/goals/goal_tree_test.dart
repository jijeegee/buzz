import 'package:buzz/features/goals/goal_provider.dart';
import 'package:buzz/features/goals/goal_tree.dart';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

const _editor =
    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const _thread =
    'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';

GoalTree _sample() {
  var tree = GoalTree.empty;
  GoalTree apply(GoalOp op) => tree = tree.apply(op, editor: _editor, now: 1);
  apply(const SetRootGoal(id: 'root', title: 'Ship it'));
  apply(const AddGoal(id: 'a', parent: 'root', title: 'A'));
  apply(const AddGoal(id: 'b', parent: 'root', title: 'B'));
  apply(const AddGoal(id: 'a1', parent: 'a', title: 'A1'));
  return tree;
}

/// Pinned in `crates/buzz-core/src/goal_tree.rs` too: the relay must accept
/// exactly what this client writes.
const _relayFixture =
    r'{"v":1,"nodes":[{"id":"root","parent":null,"title":"Ship it","note":"Key info","status":"open","order":0,"updated_by":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","updated_at":1700000000},{"id":"a","parent":"root","title":"A","status":"in_progress","threads":["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],"order":0,"updated_by":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","updated_at":1700000000}]}';

void main() {
  test('links exactly like buzz-core (shared fixture)', () {
    final fixture =
        jsonDecode(
              File(
                '../crates/buzz-core/fixtures/goal_link_fixture.json',
              ).readAsStringSync(),
            )
            as Map<String, dynamic>;
    var tree = GoalTree.parse(fixture['before'] as String);
    for (final raw in fixture['ops'] as List) {
      final op = raw as Map<String, dynamic>;
      expect(op['op'], 'link');
      tree = tree.apply(
        LinkThread(id: op['id'] as String, thread: op['thread'] as String),
        editor: fixture['editor'] as String,
        now: fixture['now'] as int,
      );
    }
    expect(tree.toContent(), fixture['after']);
  });

  test('writes the format the relay validates', () {
    var tree = GoalTree.empty;
    for (final op in <GoalOp>[
      const SetRootGoal(id: 'root', title: 'Ship it', note: 'Key info'),
      const AddGoal(id: 'a', parent: 'root', title: 'A'),
      const UpdateGoal(id: 'a', status: GoalStatus.inProgress),
      LinkThread(id: 'a', thread: _thread.toUpperCase()),
    ]) {
      tree = tree.apply(op, editor: _editor, now: 1700000000);
    }
    expect(tree.toContent(), _relayFixture);
  });

  test('outline is depth-first with layers', () {
    expect(_sample().outline().map((row) => '${row.layer}:${row.node.id}'), [
      '1:root',
      '2:a',
      '3:a1',
      '2:b',
    ]);
  });

  test('round-trips through the wire format', () {
    final tree = _sample();
    final parsed = GoalTree.parse(tree.toContent());
    expect(parsed.outline().map((r) => r.node.id), ['root', 'a', 'a1', 'b']);
    expect(parsed.node('a1')?.updatedBy, _editor);
  });

  test('parses relay-written trees with omitted optional fields', () {
    final tree = GoalTree.parse(
      '{"v":1,"nodes":[{"id":"r","parent":null,"title":"Root"}]}',
    );
    expect(tree.root?.status, GoalStatus.open);
  });

  test('rejects broken trees and edits', () {
    expect(
      () => GoalTree.parse(
        '{"v":1,"nodes":[{"id":"a","title":"A"},{"id":"b","title":"B"}]}',
      ),
      throwsA(isA<GoalTreeException>()),
    );
    final tree = _sample();
    expect(
      () => tree.apply(
        const RemoveGoal(id: 'a', recursive: false),
        editor: _editor,
        now: 2,
      ),
      throwsA(isA<GoalTreeException>()),
    );
    expect(
      () => tree.apply(
        const UpdateGoal(id: 'a', title: 'one\ntwo'),
        editor: _editor,
        now: 2,
      ),
      throwsA(isA<GoalTreeException>()),
    );
  });

  test('progress, removal, and thread links', () {
    var tree = _sample().apply(
      const UpdateGoal(id: 'a1', status: GoalStatus.done),
      editor: _editor,
      now: 2,
    );
    expect(tree.progress('root'), (done: 1, total: 3));
    tree = tree.apply(
      const LinkThread(id: 'a', thread: _thread),
      editor: _editor,
      now: 3,
    );
    tree = tree.apply(
      const LinkThread(id: 'b', thread: _thread),
      editor: _editor,
      now: 4,
    );
    expect(tree.forThread(_thread)?.id, 'b');
    expect(tree.node('a')?.threads, isEmpty);
    tree = tree.apply(
      const RemoveGoal(id: 'root', recursive: true),
      editor: _editor,
      now: 5,
    );
    expect(tree.nodes, isEmpty);
  });

  test('write tags carry the head revision or none', () {
    expect(buildGoalTreeTags(channelId: 'c', revision: null), [
      ['h', 'c'],
      ['expected-revision', 'none'],
    ]);
    expect(isGoalTreeConflict(Exception('conflict: goal tree changed')), true);
    expect(isGoalTreeConflict(Exception('conflict: canvas changed')), false);
  });
}
