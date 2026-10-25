part of '../thread_detail_page.dart';

/// Where a thread page is in opening its replies at a reply.
enum _ThreadOpenPhase {
  /// The replies open at the newest one, as they always did.
  none,

  /// Waiting for the window around the reply; the list is out of sight.
  opening,

  /// The window is in; the list settles its first position out of sight.
  positioning,

  /// The list shows the window opened at the reply.
  opened,
}

/// The reply a thread page opens at instead of the newest one.
class _ThreadOpenTarget {
  const _ThreadOpenTarget({
    required this.id,
    this.createdAt,
    this.isUnread = false,
    this.edge,
  });

  final String id;

  /// Known up front for unread replies and saved positions; a linked reply
  /// is looked up among the replies at hand.
  final int? createdAt;

  /// The first unread reply: it gets the "New messages" divider and sits at
  /// the top of the list.
  final bool isUnread;

  /// Where a saved position had its reply, when the target is one.
  final double? edge;
}

/// The list a thread window is drawn in: a new [epoch] draws a new list at
/// [index] and [alignment] from its first frame.
class _ThreadListPlacement {
  const _ThreadListPlacement({
    required this.generation,
    required this.epoch,
    required this.index,
    required this.alignment,
  });

  final int generation;
  final int epoch;
  final int index;
  final double alignment;
}

/// The reversed list index of the reply [id] among [replies].
int? _reversedReplyIndex(List<TimelineMessage> replies, String id) {
  final chronologicalIndex = replies.indexWhere((reply) => reply.id == id);
  return chronologicalIndex < 0
      ? null
      : replies.length - 1 - chronologicalIndex;
}

/// The reversed list index the window was opened at: its reply, or the
/// oldest reply not older than it.
int _threadOpenedAtIndex(
  List<TimelineMessage> replies,
  String id,
  int createdAt,
) {
  final byId = _reversedReplyIndex(replies, id);
  if (byId != null) return byId;
  final chronologicalIndex = replies.indexWhere(
    (reply) => reply.createdAt >= createdAt,
  );
  return chronologicalIndex < 0 ? 0 : replies.length - 1 - chronologicalIndex;
}

/// A placement that keeps the row being read where it is on screen after
/// replies were added below [rendered], now [replies]. The head closes the
/// top end of both lists.
_ThreadListPlacement? _reanchoredThreadPlacement(
  _ThreadListPlacement placement,
  Iterable<ItemPosition> positions,
  List<TimelineMessage> rendered,
  List<TimelineMessage> replies,
) {
  final visible = positions
      .where(
        (position) =>
            position.index <= rendered.length &&
            position.itemLeadingEdge < 1 &&
            position.itemTrailingEdge > 0,
      )
      .toList();
  if (visible.isEmpty) return null;
  // The newest row changes as rows join it (its time label), so the
  // reference is the next one up when it is on screen.
  final above = visible.where((position) => position.index > 0).toList();
  final reference = (above.isEmpty ? visible : above).reduce(
    (a, b) => a.index < b.index ? a : b,
  );
  final index = reference.index == rendered.length
      ? replies.length
      : _reversedReplyIndex(
          replies,
          rendered[rendered.length - 1 - reference.index].id,
        );
  if (index == null) return null;
  return _ThreadListPlacement(
    generation: placement.generation,
    epoch: placement.epoch + 1,
    index: index,
    alignment: reference.itemLeadingEdge,
  );
}

/// The reply a reader is on: the lowest one wholly above the list's
/// [bottom] boundary, or the lowest one in sight.
RoomPositionReading? _threadReadingPosition(
  Iterable<ItemPosition> positions,
  List<TimelineMessage> replies, {
  required double bottom,
}) {
  final visible = positions
      .where(
        (position) =>
            position.index < replies.length &&
            position.itemLeadingEdge < 1 &&
            position.itemTrailingEdge > 0,
      )
      .toList();
  if (visible.isEmpty) return null;
  final above = visible
      .where((position) => position.itemLeadingEdge >= bottom - 0.01)
      .toList();
  final reading = (above.isEmpty ? visible : above).reduce(
    (a, b) => a.index < b.index ? a : b,
  );
  final reply = replies[replies.length - 1 - reading.index];
  return RoomPositionReading.at(
    id: reply.id,
    createdAt: reply.createdAt,
    edge: reading.itemLeadingEdge,
  );
}

/// Keeps [dock] at the composer dock's measured [height]. Null until the
/// first report, and the list stays out of sight until then: a list shown
/// first without the dock's padding would move every reply when it came.
void _setDockHeight(ValueNotifier<double?> dock, double height) {
  final previous = dock.value;
  if (previous != null && (height - previous).abs() < 0.5) return;
  dock.value = height;
}

/// Whether the read marker is known: the one kept on the device counts
/// before the server's arrives, and a later one keeps the opened position.
bool _readAtKnown(ReadStateState readState, String channelId) =>
    readState.isReady || readState.effectiveTimestamp(channelId) != null;

/// An open the relay did not answer, while connected, waits out of sight
/// and asks again after [_threadOpenRetryDelay]: the newest replies are
/// never drawn first. Offline they show as before.
const _threadOpenRetryDelay = Duration(seconds: 8);

typedef _ThreadOpenRetry = ({
  int retries,
  void Function(ValueNotifier<_ThreadOpenPhase>, ObjectRef<bool>) failed,
});

_ThreadOpenRetry _useThreadOpenRetry(WidgetRef ref) {
  final retries = useState(0);
  final timer = useRef<Timer?>(null);
  useEffect(
    () =>
        () => timer.value?.cancel(),
    const [],
  );
  void failed(
    ValueNotifier<_ThreadOpenPhase> phase,
    ObjectRef<bool> didStartOpening,
  ) {
    if (ref.read(relaySessionProvider).status != SessionStatus.connected) {
      phase.value = _ThreadOpenPhase.none;
      return;
    }
    timer.value?.cancel();
    timer.value = Timer(_threadOpenRetryDelay, () {
      if (phase.value != _ThreadOpenPhase.opening) return;
      didStartOpening.value = false;
      retries.value++;
    });
  }

  return (retries: retries.value, failed: failed);
}
