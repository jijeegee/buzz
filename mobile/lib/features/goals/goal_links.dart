import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import 'goal_tree.dart';

/// What a goal's linked id is. Links share one list: a main-timeline message
/// with replies is a thread, one without is a message, and a reply is a
/// message inside its thread. [missing] covers deleted or unknown events.
enum GoalLinkKind { thread, message, reply, missing }

/// [threadRootId] is set for replies.
typedef GoalLinkInfo = ({GoalLinkKind kind, String? threadRootId});

/// Where a linked id opens: a thread alone, a reply inside its thread, or a
/// main-timeline message (no [threadRootId]).
typedef GoalLinkTarget = ({String? threadRootId, String? messageId});

/// Label and destination of a linked id. Unknown links (still loading) act
/// as threads, as they did before message links; missing ones go nowhere.
({String label, GoalLinkTarget? target}) goalLinkView(
  String id,
  GoalLinkInfo? info,
) => switch (info?.kind) {
  GoalLinkKind.message => (
    label: 'Message',
    target: (threadRootId: null, messageId: id),
  ),
  GoalLinkKind.reply => (
    label: 'Message',
    target: (threadRootId: info!.threadRootId, messageId: id),
  ),
  GoalLinkKind.missing => (label: 'Message', target: null),
  _ => (label: 'Thread', target: (threadRootId: id, messageId: null)),
};

/// Which goals a message may be linked to from its actions.
typedef GoalLinkScope = ({
  bool Function(String id, int layer) canLink,
  bool Function(String id) canAddUnder,
});

/// In a thread a message may link only the thread's goal and its sub-goals,
/// otherwise any goal from layer 2 down: layer 1 takes no links. New goals may
/// be added under any goal whose child would be linkable.
GoalLinkScope messageGoalLinkScope(GoalTree tree, String? threadRootId) {
  final threadGoal = threadRootId == null ? null : tree.forThread(threadRootId);
  if (threadGoal != null) {
    final editable = tree.subtreeIds(threadGoal.id);
    return (
      canLink: (id, _) => editable.contains(id),
      canAddUnder: editable.contains,
    );
  }
  return (canLink: (_, layer) => layer >= 2, canAddUnder: (_) => true);
}

/// Classify [ids] by reading each event: a reply names its thread root; a
/// top-level message is a thread once it has a reply. An id that does not
/// come back is still a thread when a reply names it as root (a root hidden
/// from a reader who joined after it); else it is missing, like one in
/// another channel.
Future<Map<String, GoalLinkInfo>> classifyGoalLinks(
  String channelId,
  List<String> ids,
  Future<List<NostrEvent>> Function(List<NostrFilter>) query,
) async {
  if (ids.isEmpty) return const {};
  final events = {
    for (final event in await query([
      NostrFilter(
        kinds: EventKind.channelTimelineContentKinds,
        ids: ids,
        limit: ids.length,
      ),
    ]))
      event.id.toLowerCase(): event,
  };
  final result = <String, GoalLinkInfo>{};
  final tops = <String>[];
  final unknown = <String>[];
  for (final id in ids) {
    final event = events[id.toLowerCase()];
    final rootId = event?.threadReference.rootId;
    if (event == null) {
      unknown.add(id);
    } else if (event.channelId != channelId) {
      result[id] = (kind: GoalLinkKind.missing, threadRootId: null);
    } else if (rootId != null) {
      result[id] = (kind: GoalLinkKind.reply, threadRootId: rootId);
    } else {
      tops.add(id);
    }
  }
  final probed = [...tops, ...unknown];
  if (probed.isNotEmpty) {
    // One chat reply per top-level message is enough to call it a thread;
    // job and stream events must not take the single slot.
    final replies = await query([
      for (final id in probed)
        NostrFilter(
          kinds: const [EventKind.streamMessage, EventKind.streamMessageV2],
          tags: {
            '#e': [id],
            '#h': [channelId],
          },
          limit: 1,
        ),
    ]);
    final replied = {
      for (final reply in replies)
        if (reply.threadReference.rootId case final root?) root.toLowerCase(),
    };
    for (final id in tops) {
      result[id] = (
        kind: replied.contains(id.toLowerCase())
            ? GoalLinkKind.thread
            : GoalLinkKind.message,
        threadRootId: null,
      );
    }
    for (final id in unknown) {
      result[id] = (
        kind: replied.contains(id.toLowerCase())
            ? GoalLinkKind.thread
            : GoalLinkKind.missing,
        threadRootId: null,
      );
    }
  }
  return result;
}

/// What each id linked in a conversation's goal tree is. Keyed by the
/// channel and its sorted, comma-joined linked ids.
final goalLinkKindsProvider = FutureProvider.autoDispose
    .family<Map<String, GoalLinkInfo>, ({String channelId, String ids})>((
      ref,
      args,
    ) {
      final session = ref.read(relaySessionProvider.notifier);
      return classifyGoalLinks(
        args.channelId,
        args.ids.isEmpty ? const [] : args.ids.split(','),
        session.queryRelay,
      );
    });

/// The [goalLinkKindsProvider] key for [tree]'s links in [channelId].
({String channelId, String ids}) goalLinkKindsKey(
  String channelId,
  GoalTree tree,
) => (
  channelId: channelId,
  ids: ({
    for (final node in tree.nodes) ...node.links,
  }.toList()..sort()).join(','),
);
