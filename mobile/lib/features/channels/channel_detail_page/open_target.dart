part of '../channel_detail_page.dart';

/// Where a channel page is in opening its timeline at a message.
enum _OpenPhase {
  /// The timeline opens at the newest rows, as it always did.
  none,

  /// Waiting for the window around the message; nothing is drawn yet.
  opening,

  /// The window is in; the list settles its first position out of sight.
  positioning,

  /// The list shows the window opened at the message.
  opened,
}

/// The messages a page may open at; the oldest one found is used.
class _OpenTarget {
  const _OpenTarget({
    required this.ids,
    required this.isUnread,
    this.knownCreatedAt = const {},
    this.edge,
  });

  final Set<String> ids;
  final bool isUnread;
  final Map<String, int> knownCreatedAt;

  /// Where a saved position had its row, when the target is one.
  final double? edge;
}

/// The message the timeline window was opened at.
class _OpenedAt {
  const _OpenedAt({
    required this.messageId,
    required this.createdAt,
    required this.isUnread,
    this.edge,
  });

  final String messageId;
  final int createdAt;

  /// The first unread message: it gets the "New messages" divider and sits
  /// at the top of the timeline rather than near its middle.
  final bool isUnread;

  /// A saved position's row goes back where it was: its bottom edge as a
  /// fraction of the timeline's height from the bottom.
  final double? edge;
}

/// How long an open that the relay did not answer waits before asking again,
/// out of sight; the newest rows are never drawn first.
const _openRetryDelay = Duration(seconds: 8);

/// The first main-timeline row after [readAt] that the message database
/// proves unread, counted as the unread badge counts: messages by others,
/// not in a muted room, newer than their own read marker. Null when the
/// database proves none or does not know.
NostrEvent? _storedFirstUnread(
  WidgetRef ref,
  String channelId,
  int readAt,
  int? Function(String messageId) messageMarker,
) {
  try {
    final scope = ref.read(messageScopeProvider);
    final me = ref.read(myPubkeyProvider);
    if (scope == null || me == null) return null;
    final muted =
        ref.read(channelMutesProvider).store.channels[channelId]?.muted ??
        false;
    final first = ref
        .read(channelRoomStoreProvider)
        .readFirstAfter(
          scope,
          channelId,
          readAt,
          (row) =>
              row.createdAt > (messageMarker(row.id) ?? 0) &&
              shouldNotifyForEvent(
                row,
                me,
                mutedChannelIds: {if (muted) channelId},
                channelId: channelId,
              ),
        );
    return first.known ? first.row : null;
  } catch (error) {
    debugPrint('[ChannelDetailPage] stored unread unknown: $error');
    return null;
  }
}

/// The older of the first unread the channel list observed and the one the
/// database proves.
({String id, int createdAt})? _olderUnread(
  ObservedUnreadEvent? observed,
  NostrEvent? stored,
) =>
    stored != null &&
        (observed == null || stored.createdAt < observed.createdAt)
    ? (id: stored.id, createdAt: stored.createdAt)
    : observed == null
    ? null
    : (id: observed.id, createdAt: observed.createdAt);

/// When the message database has [id] on [channelId]'s main timeline.
int? _storedCreatedAt(WidgetRef ref, String channelId, String id) {
  try {
    final scope = ref.read(messageScopeProvider);
    if (scope == null) return null;
    return ref.read(channelRoomStoreProvider).createdAtOf(scope, channelId, id);
  } catch (_) {
    return null;
  }
}

/// Whether a room painted from stored rows waits out of sight for the relay:
/// connected, nothing to open at, and the channel list knows of a message
/// newer than every stored row. Drawing the stored rows would let the missed
/// ones grow in under them, so the room waits one round trip (at most
/// [_openRetryDelay]) and appears complete.
bool _useNewerRowsHold({
  required ChannelMessagesNotifier notifier,
  required bool hasOpenTarget,
  required bool connected,
  required DateTime? listedLastMessageAt,
  required List<NostrEvent>? events,
}) {
  final holds = useRef<bool?>(null);
  if (holds.value == null) {
    var newest = 0;
    for (final event in events ?? const <NostrEvent>[]) {
      if (event.threadReference.parentId == null && event.createdAt > newest) {
        newest = event.createdAt;
      }
    }
    final listed = listedLastMessageAt == null
        ? null
        : dateTimeToUnixSeconds(listedLastMessageAt);
    holds.value =
        connected &&
        !hasOpenTarget &&
        notifier.isShowingCachedHead &&
        listed != null &&
        listed > newest;
  }
  final timedOut = useState(false);
  useEffect(() {
    if (holds.value != true) return null;
    final timer = Timer(_openRetryDelay, () => timedOut.value = true);
    return timer.cancel;
  }, const []);
  if (holds.value == true &&
      (!notifier.isShowingCachedHead || timedOut.value)) {
    holds.value = false;
  }
  return holds.value!;
}
