import 'dart:convert';
import 'dart:math';

/// Goal tree (kind 40110) — mirrors `buzz_core::goal_tree`.
///
/// One document per channel or DM. The root is the conversation's single
/// layer 1 goal; every other goal hangs below one parent, so a goal's layer
/// is its depth. The relay validates every write with the same rules.
const int kindGoalTree = 40110;
const int goalTreeVersion = 1;
const int maxGoalNodes = 500;
const int maxGoalDepth = 128;
const int maxGoalTitleChars = 200;
const int maxGoalNoteChars = 4000;
const int maxGoalContentBytes = 64 * 1024;

enum GoalStatus {
  open('open', 'To do'),
  inProgress('in_progress', 'In progress'),
  done('done', 'Done'),
  dropped('dropped', 'Dropped');

  const GoalStatus(this.wire, this.label);
  final String wire;
  final String label;

  static GoalStatus parse(String? value) => GoalStatus.values.firstWhere(
    (status) => status.wire == value,
    orElse: () => GoalStatus.open,
  );

  GoalStatus get next => GoalStatus.values[(index + 1) % values.length];
}

class GoalNode {
  const GoalNode({
    required this.id,
    required this.parent,
    required this.title,
    this.note = '',
    this.status = GoalStatus.open,
    this.assignees = const [],
    this.threads = const [],
    this.order = 0,
    this.updatedBy,
    this.updatedAt,
  });

  final String id;
  final String? parent;
  final String title;
  final String note;
  final GoalStatus status;
  final List<String> assignees;
  final List<String> threads;
  final int order;
  final String? updatedBy;
  final int? updatedAt;

  factory GoalNode.fromJson(Map<String, dynamic> json) => GoalNode(
    id: json['id'] as String,
    parent: json['parent'] as String?,
    title: json['title'] as String,
    note: (json['note'] as String?) ?? '',
    status: GoalStatus.parse(json['status'] as String?),
    assignees: [...?(json['assignees'] as List?)?.cast<String>()],
    threads: [...?(json['threads'] as List?)?.cast<String>()],
    order: (json['order'] as num?)?.toInt() ?? 0,
    updatedBy: json['updated_by'] as String?,
    updatedAt: (json['updated_at'] as num?)?.toInt(),
  );

  Map<String, dynamic> toJson() => {
    'id': id,
    'parent': parent,
    'title': title,
    if (note.isNotEmpty) 'note': note,
    'status': status.wire,
    if (assignees.isNotEmpty) 'assignees': assignees,
    if (threads.isNotEmpty) 'threads': threads,
    'order': order,
    if (updatedBy != null) 'updated_by': updatedBy,
    if (updatedAt != null) 'updated_at': updatedAt,
  };

  GoalNode copyWith({
    String? parent,
    String? title,
    String? note,
    GoalStatus? status,
    List<String>? threads,
    int? order,
    String? updatedBy,
    int? updatedAt,
  }) => GoalNode(
    id: id,
    parent: parent ?? this.parent,
    title: title ?? this.title,
    note: note ?? this.note,
    status: status ?? this.status,
    assignees: assignees,
    threads: threads ?? this.threads,
    order: order ?? this.order,
    updatedBy: updatedBy ?? this.updatedBy,
    updatedAt: updatedAt ?? this.updatedAt,
  );
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

  factory GoalTree.parse(String content) {
    final Object? decoded;
    try {
      decoded = jsonDecode(content);
    } on FormatException catch (e) {
      throw GoalTreeException('goal tree is not valid JSON: ${e.message}');
    }
    if (decoded is! Map<String, dynamic>) {
      throw const GoalTreeException('goal tree is not an object');
    }
    if (decoded['v'] != goalTreeVersion) {
      throw GoalTreeException('unsupported goal tree version ${decoded['v']}');
    }
    final tree = GoalTree([
      for (final raw in (decoded['nodes'] as List?) ?? const [])
        GoalNode.fromJson(raw as Map<String, dynamic>),
    ]);
    tree.validate();
    return tree;
  }

  String toContent() {
    validate();
    final content = jsonEncode({
      'v': goalTreeVersion,
      'nodes': [for (final node in nodes) node.toJson()],
    });
    if (utf8.encode(content).length > maxGoalContentBytes) {
      throw const GoalTreeException('The goals are too large to save.');
    }
    return content;
  }

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

  List<GoalNode> children(String id) =>
      nodes.where((node) => node.parent == id).toList()..sort(
        (a, b) => a.order != b.order
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

  ({int done, int total}) progress(String id) {
    final below = subtreeIds(id)..remove(id);
    final done = below.where((g) => node(g)?.status == GoalStatus.done).length;
    return (done: done, total: below.length);
  }

  GoalNode? forThread(String threadRootId) {
    final thread = threadRootId.toLowerCase();
    for (final node in nodes) {
      if (node.threads.contains(thread)) return node;
    }
    return null;
  }

  void validate() {
    if (nodes.length > maxGoalNodes) {
      throw const GoalTreeException('Too many goals.');
    }
    final ids = <String>{};
    final threads = <String>{};
    var roots = 0;
    for (final node in nodes) {
      if (!RegExp(r'^[A-Za-z0-9_-]{1,64}$').hasMatch(node.id)) {
        throw GoalTreeException('invalid goal id ${node.id}');
      }
      if (!ids.add(node.id)) {
        throw GoalTreeException('duplicate goal id ${node.id}');
      }
      final titleError = goalTitleError(node.title);
      if (titleError != null) throw GoalTreeException(titleError);
      if (node.note.runes.length > maxGoalNoteChars) {
        throw const GoalTreeException('Keep the note shorter.');
      }
      for (final thread in node.threads) {
        if (!threads.add(thread)) {
          throw const GoalTreeException('A thread can work on only one goal.');
        }
      }
      if (node.parent == null) roots++;
    }
    if (nodes.isNotEmpty && roots != 1) {
      throw const GoalTreeException('There must be exactly one layer 1 goal.');
    }
    final parents = {for (final node in nodes) node.id: node.parent};
    for (final node in nodes) {
      final parent = node.parent;
      if (parent != null && !parents.containsKey(parent)) {
        throw GoalTreeException('goal ${node.id} has an unknown parent');
      }
      var depth = 1;
      var cursor = parent;
      while (cursor != null) {
        depth++;
        if (depth > maxGoalDepth) {
          throw const GoalTreeException('Goals are nested too deeply.');
        }
        cursor = parents[cursor];
      }
    }
  }

  /// Apply one edit and validate the result. Throws without changing
  /// anything when the edit is not allowed.
  GoalTree apply(GoalOp op, {required String editor, required int now}) {
    final next = op._apply(this, editor.toLowerCase(), now);
    next.validate();
    return next;
  }

  GoalTree _replace(String id, GoalNode Function(GoalNode) update) {
    if (node(id) == null) throw GoalTreeException('goal $id not found');
    return GoalTree([
      for (final node in nodes) node.id == id ? update(node) : node,
    ]);
  }

  int _nextOrder(String parent) {
    final siblings = children(parent);
    return siblings.isEmpty ? 0 : siblings.last.order + 1;
  }
}

String? goalTitleError(String title) {
  if (title.trim().isEmpty) return 'Write the goal in one sentence.';
  if (title.contains('\n') || title.contains('\r')) {
    return 'Keep the goal on one line.';
  }
  if (title.runes.length > maxGoalTitleChars) {
    return 'Keep the goal under $maxGoalTitleChars characters.';
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

/// One edit; mirrors `buzz_core::goal_tree::GoalOp`. Writers keep the op so
/// they can re-apply it to a newer head after a conflict.
sealed class GoalOp {
  const GoalOp();
  GoalTree _apply(GoalTree tree, String editor, int now);
}

class SetRootGoal extends GoalOp {
  const SetRootGoal({required this.id, required this.title, this.note});
  final String id;
  final String title;
  final String? note;

  @override
  GoalTree _apply(GoalTree tree, String editor, int now) {
    final root = tree.root;
    if (root != null) {
      return tree._replace(
        root.id,
        (n) => n.copyWith(
          title: title,
          note: note,
          updatedBy: editor,
          updatedAt: now,
        ),
      );
    }
    return GoalTree([
      ...tree.nodes,
      GoalNode(
        id: id,
        parent: null,
        title: title,
        note: note ?? '',
        updatedBy: editor,
        updatedAt: now,
      ),
    ]);
  }
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
  GoalTree _apply(GoalTree tree, String editor, int now) {
    if (tree.node(parent) == null) {
      throw GoalTreeException('goal $parent not found');
    }
    return GoalTree([
      ...tree.nodes,
      GoalNode(
        id: id,
        parent: parent,
        title: title,
        note: note ?? '',
        order: tree._nextOrder(parent),
        updatedBy: editor,
        updatedAt: now,
      ),
    ]);
  }
}

class UpdateGoal extends GoalOp {
  const UpdateGoal({required this.id, this.title, this.note, this.status});
  final String id;
  final String? title;
  final String? note;
  final GoalStatus? status;

  @override
  GoalTree _apply(GoalTree tree, String editor, int now) => tree._replace(
    id,
    (n) => n.copyWith(
      title: title,
      note: note,
      status: status,
      updatedBy: editor,
      updatedAt: now,
    ),
  );
}

class RemoveGoal extends GoalOp {
  const RemoveGoal({required this.id, required this.recursive});
  final String id;
  final bool recursive;

  @override
  GoalTree _apply(GoalTree tree, String editor, int now) {
    if (tree.node(id) == null) throw GoalTreeException('goal $id not found');
    final doomed = tree.subtreeIds(id);
    if (doomed.length > 1 && !recursive) {
      throw const GoalTreeException('Remove its sub-goals too.');
    }
    return GoalTree([
      for (final node in tree.nodes)
        if (!doomed.contains(node.id)) node,
    ]);
  }
}

class LinkThread extends GoalOp {
  const LinkThread({required this.id, required this.thread});
  final String id;
  final String thread;

  @override
  GoalTree _apply(GoalTree tree, String editor, int now) {
    final thread = this.thread.toLowerCase();
    if (tree.node(id) == null) throw GoalTreeException('goal $id not found');
    return GoalTree([
      for (final node in tree.nodes)
        node.id == id
            ? node.copyWith(
                threads: [...node.threads.where((t) => t != thread), thread],
                updatedBy: editor,
                updatedAt: now,
              )
            : node.copyWith(
                threads: node.threads.where((t) => t != thread).toList(),
              ),
    ]);
  }
}
