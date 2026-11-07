import 'dart:convert';

import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/relay/relay.dart';
import 'goal_tree.dart';

/// A conversation's active goals and how many goals each list holds.
class GoalBoard {
  const GoalBoard({required this.tree, required this.counts});

  final GoalTree tree;
  final GoalCounts counts;
}

Map<String, dynamic> goalViewFilter(
  String channelId,
  GoalView view, {
  Object? cursor,
}) => {'goals': 1, 'channel': channelId, 'view': view.name, 'cursor': ?cursor};

Map<String, dynamic> goalThreadsFilter(
  String channelId,
  List<String> threads,
) => {'goals': 1, 'channel': channelId, 'threads': threads};

Future<GoalRead> _readGoals(Ref ref, Map<String, dynamic> filter) async {
  final session = ref.read(relaySessionProvider.notifier);
  final body = await session.queryRelayJson([filter]);
  return GoalRead.fromJson(body as Map<String, dynamic>);
}

final goalTreeProvider = FutureProvider.family<GoalBoard, String>((
  ref,
  channelId,
) async {
  ref.watch(myPubkeyProvider);
  ref.watch(relaySessionProvider.notifier);
  var read = await _readGoals(ref, goalViewFilter(channelId, GoalView.active));
  for (var cursor = read.next; cursor != null; cursor = read.next) {
    read = read.followedBy(
      await _readGoals(
        ref,
        goalViewFilter(channelId, GoalView.active, cursor: cursor),
      ),
    );
  }
  return GoalBoard(tree: read.tree, counts: read.counts);
});

/// The threads or messages a read asks about: their ids lowercase and
/// comma-joined, so a record of them keys a provider.
typedef ThreadGoalsKey = ({String channelId, String threads});

ThreadGoalsKey threadGoalsKey(String channelId, Iterable<String> threads) => (
  channelId: channelId,
  threads: threads.map((id) => id.toLowerCase()).join(','),
);

/// The active goals plus what these threads or messages work on, whatever
/// list their goals are in. Re-reads with [goalTreeProvider].
final threadGoalTreeProvider = FutureProvider.autoDispose
    .family<GoalTree, ThreadGoalsKey>((ref, key) async {
      final board = await ref.watch(goalTreeProvider(key.channelId).future);
      final read = await _readGoals(
        ref,
        goalThreadsFilter(key.channelId, key.threads.split(',')),
      );
      return mergeGoalTrees([board.tree, read.tree]);
    });

/// A folded list of a conversation.
typedef GoalListKey = ({String channelId, GoalView view});

/// The pages read of a quiet or done list. Read when the list opens, and
/// re-read, as many pages as were shown, with [goalTreeProvider].
class GoalListNotifier extends AsyncNotifier<GoalRead> {
  GoalListNotifier(this.key);
  final GoalListKey key;
  var _pages = 1;

  Future<GoalRead> _page(Object? cursor) =>
      _readGoals(ref, goalViewFilter(key.channelId, key.view, cursor: cursor));

  @override
  Future<GoalRead> build() async {
    await ref.watch(goalTreeProvider(key.channelId).future);
    var read = await _page(null);
    for (var page = 1; page < _pages && read.next != null; page++) {
      read = read.followedBy(await _page(read.next));
    }
    return read;
  }

  Future<void> loadMore() async {
    final current = state.value;
    final cursor = current?.next;
    if (current == null || cursor == null) return;
    final page = await _page(cursor);
    // A re-read got in first; its rows are the newer ones.
    if (!identical(state.value, current)) return;
    _pages++;
    state = AsyncData(current.followedBy(page));
  }
}

final goalListProvider = AsyncNotifierProvider.autoDispose
    .family<GoalListNotifier, GoalRead, GoalListKey>(GoalListNotifier.new);

/// Keeps [goalTreeProvider] of [channelId] live while a room or thread screen
/// shows it: a new edit or a reconnect re-reads it. Rides the open-room REQ
/// ([RoomLiveRegistry]), so it adds no REQ of its own, and closes with the
/// last screen.
final goalTreeLiveProvider = Provider.autoDispose.family<void, String>((
  ref,
  channelId,
) {
  void refresh() => ref.invalidate(goalTreeProvider(channelId));
  final stop = ref
      .watch(roomLiveRegistryProvider)
      .listen(
        channelId,
        roomLiveGoalOpsKind,
        (_) => refresh(),
        onResync: refresh,
      );
  ref.onDispose(stop);
});

/// Sends goal edits as one event; the relay applies them in order, all or
/// none, so there is nothing to re-read or retry here.
class GoalActions {
  GoalActions(this._ref);
  final Ref _ref;

  Future<void> apply(String channelId, GoalOp op) => applyAll(channelId, [op]);

  Future<void> applyAll(String channelId, List<GoalOp> ops) async {
    final signer = SignedEventRelay.forConfig(
      session: _ref.read(relaySessionProvider.notifier),
      config: _ref.read(relayConfigProvider),
    );
    if (signer.pubkey == null) {
      throw GoalTreeException(appL10n.goalsSignInToEdit);
    }
    await signer.submit(
      kind: kindGoalOps,
      content: jsonEncode({
        'ops': [for (final op in ops) op.toJson()],
      }),
      tags: [
        ['h', channelId],
      ],
    );
    _ref.invalidate(goalTreeProvider(channelId));
  }
}

final goalActionsProvider = Provider<GoalActions>(GoalActions.new);
