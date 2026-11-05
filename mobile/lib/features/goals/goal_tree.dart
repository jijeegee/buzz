import 'dart:math';

import '../../shared/l10n/l10n.dart';

/// Goal edits (kind 40111) and the rows the relay reads back. The relay holds
/// the goals and applies every rule; this is the display model and the ops.
const int kindGoalOps = 40111;
const int maxGoalDepth = 128;
const int maxGoalTitleChars = 200;

enum GoalStatus {
  open('open'),
  inProgress('in_progress'),
  done('done'),
  dropped('dropped');

  const GoalStatus(this.wire);
  final String wire;

  /// The status as a goal's status button names it.
  String label(AppLocalizations l10n) => switch (this) {
    open => l10n.goalsStatusOpen,
    inProgress => l10n.goalsStatusInProgress,
    done => l10n.goalsStatusDone,
    dropped => l10n.goalsStatusDropped,
  };

  /// The goal actions menu item that sets this status.
  String markLabel(AppLocalizations l10n) => switch (this) {
    open => l10n.goalsMarkOpen,
    inProgress => l10n.goalsMarkInProgress,
    done => l10n.goalsMarkDone,
    dropped => l10n.goalsMarkDropped,
  };

  static GoalStatus parse(String? value) => GoalStatus.values.firstWhere(
    (status) => status.wire == value,
    orElse: () => GoalStatus.open,
  );

  GoalStatus get next => GoalStatus.values[(index + 1) % values.length];
}

/// One goal row, with what the relay computed for it.
class GoalNode {
  const GoalNode({
    required this.id,
    required this.parent,
    required this.title,
    this.note = '',
    this.status = GoalStatus.open,
    this.assignees = const [],
    this.links = const [],
    this.order = 0,
    this.updatedBy,
    this.updatedAt,
    this.matched = true,
    this.inactive = false,
    this.isThread = false,
    this.progress = (done: 0, total: 0),
    this.openChildren = 0,
  });

  final String id;
  final String? parent;
  final String title;
  final String note;
  final GoalStatus status;
  final List<String> assignees;

  /// Threads and messages linked to this goal.
  final List<String> links;
  final int order;
  final String? updatedBy;
  final int? updatedAt;

  /// Selected by the read, not just a goal above one that was.
  final bool matched;
  final bool inactive;

  /// Has a link that counts as a thread.
  final bool isThread;
  final ({int done, int total}) progress;
  final int openChildren;

  /// Finishing this goal as [next] deletes the tasks below it (the relay's
  /// rule).
  bool finishDeletesTasks(GoalStatus next) {
    final running =
        status == GoalStatus.open || status == GoalStatus.inProgress;
    final finishing = next == GoalStatus.done || next == GoalStatus.dropped;
    return running &&
        finishing &&
        isThread &&
        (progress.total > 0 || openChildren > 0);
  }

  GoalNode asMatched() => GoalNode(
    id: id,
    parent: parent,
    title: title,
    note: note,
    status: status,
    assignees: assignees,
    links: links,
    order: order,
    updatedBy: updatedBy,
    updatedAt: updatedAt,
    inactive: inactive,
    isThread: isThread,
    progress: progress,
    openChildren: openChildren,
  );

  factory GoalNode.fromJson(Map<String, dynamic> json) {
    final progress = json['progress'] as Map<String, dynamic>?;
    return GoalNode(
      id: json['id'] as String,
      parent: json['parent'] as String?,
      title: json['title'] as String,
      note: (json['note'] as String?) ?? '',
      status: GoalStatus.parse(json['status'] as String?),
      assignees: [...?(json['assignees'] as List?)?.cast<String>()],
      links: [...?(json['links'] as List?)?.cast<String>()],
      order: (json['order'] as num?)?.toInt() ?? 0,
      updatedBy: json['updated_by'] as String?,
      updatedAt: (json['updated_at'] as num?)?.toInt(),
      matched: (json['matched'] as bool?) ?? true,
      inactive: (json['inactive'] as bool?) ?? false,
      isThread: (json['is_thread'] as bool?) ?? false,
      progress: (
        done: (progress?['done'] as num?)?.toInt() ?? 0,
        total: (progress?['total'] as num?)?.toInt() ?? 0,
      ),
      openChildren: (json['open_children'] as num?)?.toInt() ?? 0,
    );
  }
}

class GoalTreeException implements Exception {
  const GoalTreeException(this.message);
  final String message;
  @override
  String toString() => message;
}

/// A layered row of the outline.
typedef GoalRow = ({int layer, GoalNode node});

class GoalTree {
  const GoalTree(this.nodes);
  static const empty = GoalTree([]);

  final List<GoalNode> nodes;

  GoalNode? get root {
    for (final node in nodes) {
      if (node.parent == null) return node;
    }
    return null;
  }

  GoalNode? node(String id) {
    for (final node in nodes) {
      if (node.id == id) return node;
    }
    return null;
  }

  /// Plan goals first, thread goals below them.
  List<GoalNode> children(String id) =>
      nodes.where((node) => node.parent == id).toList()..sort(
        (a, b) => a.isThread != b.isThread
            ? (a.isThread ? 1 : -1)
            : a.order != b.order
            ? a.order.compareTo(b.order)
            : a.id.compareTo(b.id),
      );

  List<GoalRow> outline() {
    final rows = <GoalRow>[];
    void walk(GoalNode node, int layer) {
      rows.add((layer: layer, node: node));
      if (layer >= maxGoalDepth) return;
      for (final child in children(node.id)) {
        walk(child, layer + 1);
      }
    }

    final root = this.root;
    if (root != null) walk(root, 1);
    return rows;
  }

  List<GoalNode> path(String id) {
    final path = <GoalNode>[];
    var cursor = node(id);
    while (cursor != null && path.length <= maxGoalDepth) {
      path.insert(0, cursor);
      final parent = cursor.parent;
      cursor = parent == null ? null : node(parent);
    }
    return path;
  }

  Set<String> subtreeIds(String id) {
    final ids = <String>{id};
    final stack = [id];
    while (stack.isNotEmpty) {
      for (final child in children(stack.removeLast())) {
        if (ids.add(child.id)) stack.add(child.id);
      }
    }
    return ids;
  }

  /// The goals on [id]'s path, without layer 1 and without [id].
  List<GoalNode> between(String id) {
    final goals = path(id);
    return goals.length > 2 ? goals.sublist(1, goals.length - 1) : const [];
  }

  /// A goal with no thread link, here or above; it shows unfolded.
  bool isPlanGoal(String id) => !path(id).any((node) => node.isThread);

  /// The selected goals a list shows at its top level, in order. Layer 1
  /// keeps to its card: a row would offer to remove the whole room.
  List<GoalNode> listTops() =>
      nodes
          .where(
            (node) =>
                node.matched &&
                node.parent != null &&
                !between(node.id).any((goal) => goal.matched),
          )
          .toList()
        ..sort(
          (a, b) => a.order != b.order
              ? a.order.compareTo(b.order)
              : a.id.compareTo(b.id),
        );

  GoalNode? forThread(String threadRootId) {
    final thread = threadRootId.toLowerCase();
    for (final node in nodes) {
      if (node.links.contains(thread)) return node;
    }
    return null;
  }
}

/// Goals by list, as the relay counts them.
typedef GoalCounts = ({int active, int inactive, int done});

/// The lists a room's goals fall into.
enum GoalView { active, inactive, done }

/// One answer to a goals `/query`.
class GoalRead {
  const GoalRead({required this.counts, required this.tree, this.next});

  factory GoalRead.fromJson(Map<String, dynamic> json) {
    final counts = json['counts'] as Map<String, dynamic>?;
    return GoalRead(
      counts: (
        active: (counts?['active'] as num?)?.toInt() ?? 0,
        inactive: (counts?['inactive'] as num?)?.toInt() ?? 0,
        done: (counts?['done'] as num?)?.toInt() ?? 0,
      ),
      tree: GoalTree([
        for (final row in (json['rows'] as List?) ?? const [])
          GoalNode.fromJson(row as Map<String, dynamic>),
      ]),
      next: json['next'],
    );
  }

  final GoalCounts counts;
  final GoalTree tree;

  /// The cursor of the following page, or null on the last one.
  final Object? next;

  /// This answer with the page after it.
  GoalRead followedBy(GoalRead page) => GoalRead(
    counts: page.counts,
    tree: mergeGoalTrees([tree, page.tree]),
    next: page.next,
  );
}

/// Several reads as one tree. A goal in more than one shows the last, and
/// stays selected if any read selected it: a later page brings it back as the
/// parent of what it selected.
GoalTree mergeGoalTrees(List<GoalTree> trees) {
  final byId = <String, GoalNode>{};
  for (final tree in trees) {
    for (final node in tree.nodes) {
      byId[node.id] = node.matched || byId[node.id]?.matched != true
          ? node
          : node.asMatched();
    }
  }
  return GoalTree([...byId.values]);
}

String? goalTitleError(String title) {
  if (title.trim().isEmpty) return appL10n.goalsTitleEmpty;
  if (title.contains('\n') || title.contains('\r')) {
    return appL10n.goalsTitleOneLine;
  }
  if (title.runes.length > maxGoalTitleChars) {
    return appL10n.goalsTitleTooLong(maxGoalTitleChars);
  }
  return null;
}

String newGoalId() {
  final random = Random.secure();
  final hex = List.generate(
    8,
    (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
  ).join();
  return 'g_$hex';
}

/// One edit; mirrors `kuzz_core::goal_tree::GoalOp`.
sealed class GoalOp {
  const GoalOp();
  Map<String, dynamic> toJson();
}

class SetRootGoal extends GoalOp {
  const SetRootGoal({required this.id, required this.title, this.note});
  final String id;
  final String title;
  final String? note;

  @override
  Map<String, dynamic> toJson() => {
    'op': 'set_root',
    'id': id,
    'title': title,
    if (note != null) 'note': note,
  };
}

class AddGoal extends GoalOp {
  const AddGoal({
    required this.id,
    required this.parent,
    required this.title,
    this.note,
  });
  final String id;
  final String parent;
  final String title;
  final String? note;

  @override
  Map<String, dynamic> toJson() => {
    'op': 'add',
    'id': id,
    'parent': parent,
    'title': title,
    if (note != null) 'note': note,
  };
}

class UpdateGoal extends GoalOp {
  const UpdateGoal({required this.id, this.title, this.note, this.status});
  final String id;
  final String? title;
  final String? note;
  final GoalStatus? status;

  @override
  Map<String, dynamic> toJson() => {
    'op': 'update',
    'id': id,
    if (title != null) 'title': title,
    if (note != null) 'note': note,
    if (status != null) 'status': status!.wire,
  };
}

class RemoveGoal extends GoalOp {
  const RemoveGoal({required this.id, required this.recursive});
  final String id;
  final bool recursive;

  @override
  Map<String, dynamic> toJson() => {
    'op': 'remove',
    'id': id,
    'recursive': recursive,
  };
}

class LinkThread extends GoalOp {
  const LinkThread({required this.id, required this.thread});
  final String id;
  final String thread;

  @override
  Map<String, dynamic> toJson() => {
    'op': 'link',
    'id': id,
    'thread': thread.toLowerCase(),
  };
}

class UnlinkThread extends GoalOp {
  const UnlinkThread({required this.thread});
  final String thread;

  @override
  Map<String, dynamic> toJson() => {
    'op': 'unlink',
    'thread': thread.toLowerCase(),
  };
}

/// Gives up every quiet goal at once.
class DropQuietGoals extends GoalOp {
  const DropQuietGoals();

  @override
  Map<String, dynamic> toJson() => {'op': 'drop_inactive'};
}
