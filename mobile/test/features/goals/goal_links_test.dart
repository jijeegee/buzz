import 'package:kuzz/features/goals/goal_links.dart';
import 'package:kuzz/features/goals/goal_tree.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';

const _editor =
    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const _channel = 'channel-1';
final _root = 'a1' * 32;
final _lone = 'b2' * 32;
final _reply = 'c3' * 32;
final _other = 'd4' * 32;
final _gone = 'e5' * 32;

GoalTree _tree() {
  var tree = GoalTree.empty;
  for (final op in <GoalOp>[
    const SetRootGoal(id: 'root', title: 'Launch'),
    const AddGoal(id: 'srv', parent: 'root', title: 'Server'),
    const AddGoal(id: 'cas', parent: 'srv', title: 'CAS writes'),
    const AddGoal(id: 'ui', parent: 'root', title: 'UI'),
    LinkThread(id: 'srv', thread: _root),
  ]) {
    tree = tree.apply(op, editor: _editor, now: 1);
  }
  return tree;
}

NostrEvent _event(
  String id,
  String channel, [
  List<List<String>> tags = const [],
]) => NostrEvent(
  id: id,
  pubkey: _editor,
  createdAt: 1,
  kind: 9,
  tags: [
    ['h', channel],
    ...tags,
  ],
  content: '',
);

List<List<String>> _replyTo(String root) => [
  ['e', root, '', 'root'],
  ['e', root, '', 'reply'],
];

void main() {
  test('message scope: thread branch, any goal unlinked, layer 2+ on main', () {
    final tree = _tree();
    List<String> linkable(GoalLinkScope scope) => [
      for (final row in tree.outline())
        if (scope.canLink(row.node.id, row.layer)) row.node.id,
    ];
    final inLinked = messageGoalLinkScope(tree, _root);
    expect(linkable(inLinked), ['srv', 'cas']);
    expect(inLinked.canAddUnder('ui'), isFalse);
    expect(linkable(messageGoalLinkScope(tree, _lone)), contains('root'));
    final main = messageGoalLinkScope(tree, null);
    expect(linkable(main), ['srv', 'cas', 'ui']);
    expect(main.canAddUnder('root'), isTrue);
  });

  test('link view: threads open, messages jump, missing go nowhere', () {
    expect(goalLinkView(_root, null), (
      label: 'Thread',
      target: (threadRootId: _root, messageId: null),
    ));
    expect(
      goalLinkView(_lone, (kind: GoalLinkKind.message, threadRootId: null)),
      (label: 'Message', target: (threadRootId: null, messageId: _lone)),
    );
    expect(
      goalLinkView(_reply, (kind: GoalLinkKind.reply, threadRootId: _root)),
      (label: 'Message', target: (threadRootId: _root, messageId: _reply)),
    );
    expect(
      goalLinkView(_gone, (kind: GoalLinkKind.missing, threadRootId: null)),
      (label: 'Message', target: null),
    );
  });

  test('links classify as thread, message, reply, or missing', () async {
    final queries = <List<NostrFilter>>[];
    final kinds = await classifyGoalLinks(
      _channel,
      [_root, _lone, _reply, _other, _gone],
      (filters) async {
        queries.add(filters);
        if (queries.length == 1) {
          return [
            _event(_root, _channel),
            _event(_lone, _channel),
            _event(_reply, _channel, _replyTo(_root)),
            _event(_other, 'another-channel'),
          ];
        }
        // The reply lookup: only _root has one.
        return [_event('f6' * 32, _channel, _replyTo(_root))];
      },
    );
    expect(kinds, {
      _root: (kind: GoalLinkKind.thread, threadRootId: null),
      _lone: (kind: GoalLinkKind.message, threadRootId: null),
      _reply: (kind: GoalLinkKind.reply, threadRootId: _root),
      _other: (kind: GoalLinkKind.missing, threadRootId: null),
      _gone: (kind: GoalLinkKind.missing, threadRootId: null),
    });
    // Only top-level messages need a reply lookup, one filter each.
    expect(queries, hasLength(2));
    expect(queries[1].map((filter) => filter.tags['#e']!.single), [
      _root,
      _lone,
    ]);
  });
}
