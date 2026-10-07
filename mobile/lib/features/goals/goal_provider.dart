import 'dart:math';

import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import 'goal_tree.dart';

/// The live head of a conversation's goal tree.
class GoalTreeHead {
  const GoalTreeHead({
    required this.revision,
    required this.createdAt,
    required this.tree,
  });

  static const none = GoalTreeHead(
    revision: null,
    createdAt: null,
    tree: GoalTree.empty,
  );

  /// Event id of the head, or null when the conversation has no goals yet.
  final String? revision;
  final int? createdAt;
  final GoalTree tree;
}

NostrFilter goalTreeFilter(String channelId) => NostrFilter(
  kinds: const [kindGoalTree],
  tags: {
    '#h': [channelId],
  },
  limit: 1,
);

GoalTreeHead _headFrom(List<NostrEvent> events) {
  if (events.isEmpty) return GoalTreeHead.none;
  final event = events.first;
  return GoalTreeHead(
    revision: event.id,
    createdAt: event.createdAt,
    tree: GoalTree.parse(event.content),
  );
}

final goalTreeProvider = FutureProvider.family<GoalTreeHead, String>((
  ref,
  channelId,
) async {
  final session = ref.watch(relaySessionProvider.notifier);
  return _headFrom(await session.queryRelay([goalTreeFilter(channelId)]));
});

/// Tags for a goal tree write: channel scope plus the revision it builds on.
List<List<String>> buildGoalTreeTags({
  required String channelId,
  required String? revision,
}) => [
  ['h', channelId],
  ['expected-revision', revision ?? 'none'],
];

bool isGoalTreeConflict(Object error) =>
    error.toString().contains('conflict: goal tree');

/// Writes goal edits with the relay's compare-and-swap: read the head, apply
/// the op, publish with `expected-revision`, and on a conflict re-apply the
/// same op to the newer head.
class GoalActions {
  GoalActions(this._ref);
  final Ref _ref;
  static const _maxAttempts = 4;

  Future<void> apply(String channelId, GoalOp op) async {
    final relayConfig = _ref.read(relayConfigProvider);
    final session = _ref.read(relaySessionProvider.notifier);
    final signer = SignedEventRelay(session: session, nsec: relayConfig.nsec);
    final editor = signer.pubkey;
    if (editor == null) {
      throw const GoalTreeException('Sign in to edit goals.');
    }
    for (var attempt = 0; attempt < _maxAttempts; attempt++) {
      final head = _headFrom(
        await session.queryRelay([goalTreeFilter(channelId)]),
      );
      final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
      final next = head.tree.apply(op, editor: editor, now: now);
      try {
        await signer.submit(
          kind: kindGoalTree,
          content: next.toContent(),
          tags: buildGoalTreeTags(
            channelId: channelId,
            revision: head.revision,
          ),
          // Sort strictly ahead of the head we built on.
          createdAt: max(now, (head.createdAt ?? 0) + 1),
        );
        _ref.invalidate(goalTreeProvider(channelId));
        return;
      } catch (error) {
        if (!isGoalTreeConflict(error)) rethrow;
      }
    }
    _ref.invalidate(goalTreeProvider(channelId));
    throw const GoalTreeException(
      'The goals kept changing while saving. Try again.',
    );
  }
}

final goalActionsProvider = Provider<GoalActions>(GoalActions.new);
