import 'timeline_message.dart';

/// Threads are one level deep (Slack-style): "Reply in thread" is offered
/// only on messages shallower than this, so replies inside a thread stay
/// flat. Existing deeper replies still render. Raise it to allow nested
/// replies again.
const maxThreadDepth = 1;

bool canReplyInThread(int depth) => depth < maxThreadDepth;

/// Nesting depth of [message] within its thread: 0 for a top-level message,
/// 1 for a direct reply to the thread head, and so on. Walks parents through
/// [messages]; a nested reply whose parent is not loaded counts as depth 2,
/// matching the desktop fallback.
int threadDepthOf(
  TimelineMessage message, [
  Iterable<TimelineMessage> messages = const [],
]) {
  Map<String, TimelineMessage>? byId;
  final seen = <String>{};
  var depth = 0;
  TimelineMessage current = message;
  while (seen.add(current.id)) {
    final parentId = current.parentId;
    if (parentId == null) return depth;
    depth++;
    if (parentId == current.rootId) return depth;
    byId ??= {for (final m in messages) m.id: m};
    final parent = byId[parentId];
    if (parent == null) return depth + 1;
    current = parent;
  }
  return depth;
}
