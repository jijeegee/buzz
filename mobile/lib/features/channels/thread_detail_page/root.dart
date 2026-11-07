part of '../thread_detail_page.dart';

/// The page's head: the one it was pushed with, else the root as resolved
/// from its id (docs/message-sync.md#opening-a-thread-from-its-root-id),
/// else a stand-in carrying only the id while it is [resolving] or [missing]
/// (the relay answered without it). Nothing on the page waits for it.
/// [unconfirmed] while its root is still in doubt: only a
/// [ThreadDetailPage.headMayBeNested] head, until the lookup found it (drawn
/// or not), came back without it, proved it deleted, or failed. [event] is
/// the head's event when resolved here, so the thread's reactions, edits and
/// deletions of it apply.
({
  TimelineMessage head,
  NostrEvent? event,
  bool resolving,
  bool missing,
  bool deleted,
  bool unconfirmed,
})
_watchThreadHead(WidgetRef ref, ThreadDetailPage page, String? currentPubkey) {
  final passed = page.threadHead;
  if (passed != null) {
    return (
      head: passed,
      event: null,
      resolving: false,
      missing: false,
      deleted: false,
      unconfirmed: false,
    );
  }
  final root = ref.watch(
    threadRootProvider(
      ThreadRepliesArgs(channelId: page.channelId, rootId: page.headId),
    ),
  );
  final event = root.event ?? page.rootHint;
  final head = event == null
      ? null
      : formatTimeline(
          [event],
          currentPubkey: currentPubkey,
        ).where((m) => m.id == page.headId).firstOrNull;
  return (
    head:
        head ??
        TimelineMessage(
          id: page.headId,
          pubkey: '',
          createdAt: 0,
          content: '',
          rootId: page.rootId,
        ),
    event: head == null ? null : event,
    resolving: head == null && !root.missing,
    missing: head == null && root.missing,
    deleted: root.deleted,
    unconfirmed:
        page.headMayBeNested &&
        event == null &&
        !root.missing &&
        !root.failed &&
        !root.deleted,
  );
}

/// What stands in for a hidden root's tags when seeding agent pins: the
/// replies' authors and the people they tag (only agents are kept).
List<List<String>> _standInRootTags(List<TimelineMessage> replies) => [
  for (final reply in replies) ...[
    ['p', reply.pubkey],
    for (final tag in reply.tags)
      if (tag.length > 1 && tag[0] == 'p') tag,
  ],
];

/// The thread's room for sending, and whether the composer waits for it: a
/// room built from a Chats row stands in for display only, so replies need
/// the listed room or its details first (docs/message-sync.md#launch-snapshots).
({Channel? channel, bool composerWaits}) _watchThreadChannel(
  WidgetRef ref,
  ThreadDetailPage page,
) {
  final listed = ref.watch(
    channelsProvider.select(selectChannel(page.channelId)),
  );
  final standIn = page.standInChannel;
  if (listed != null || standIn == null) {
    return (channel: listed, composerWaits: false);
  }
  final details = ref.watch(channelDetailsProvider(page.channelId)).value;
  final channel = details == null ? null : standIn.mergeDetails(details);
  return (channel: channel, composerWaits: channel == null);
}

/// Follows the keyboard: the list's viewport as metrics settle, and
/// [settledInset] on Android's fixed viewport.
void _useSettledImeInset(
  LaidOutViewport listViewport,
  ValueNotifier<double> settledInset,
) {
  final appView = View.of(useContext());
  useEffect(() {
    final observer = ImeMetricsSettleObserver(
      onMetricsSettled: () {
        listViewport.reportAfterLayout();
        if (!usesFixedAndroidImeViewport) return;
        final nextInset = appView.viewInsets.bottom / appView.devicePixelRatio;
        if ((settledInset.value - nextInset).abs() >= 0.5) {
          settledInset.value = nextInset;
        }
      },
    );
    WidgetsBinding.instance.addObserver(observer);
    return () {
      WidgetsBinding.instance.removeObserver(observer);
      observer.dispose();
    };
  }, [appView, listViewport]);
}

/// Flashes the linked reply once the route has settled and it is laid out.
ValueNotifier<String?> _useLandingHighlight(
  BuildContext context,
  String? initialMessageId,
  bool targetReady,
) {
  final highlightedMessageId = useState<String?>(null);
  final routeAnimation = ModalRoute.of(context)?.animation;
  final reducedMotion = MediaQuery.disableAnimationsOf(context);
  useEffect(() {
    final messageId = initialMessageId;
    if (messageId == null || !targetReady) return null;
    var disposed = false;
    Timer? revealTimer;
    Timer? dismissTimer;

    void revealHighlight() {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (disposed) return;
        revealTimer = Timer(_landingHighlightDelay, () {
          if (disposed) return;
          highlightedMessageId.value = messageId;
          dismissTimer = Timer(
            _landingHighlightDuration +
                (reducedMotion
                    ? Duration.zero
                    : _landingHighlightTransitionDuration),
            () {
              if (!disposed) highlightedMessageId.value = null;
            },
          );
        });
      });
    }

    void handleRouteStatus(AnimationStatus status) {
      if (status != AnimationStatus.completed) return;
      routeAnimation?.removeStatusListener(handleRouteStatus);
      revealHighlight();
    }

    if (routeAnimation == null ||
        routeAnimation.status == AnimationStatus.completed) {
      revealHighlight();
    } else {
      routeAnimation.addStatusListener(handleRouteStatus);
    }

    return () {
      disposed = true;
      routeAnimation?.removeStatusListener(handleRouteStatus);
      revealTimer?.cancel();
      dismissTimer?.cancel();
    };
  }, [initialMessageId, targetReady, reducedMotion, routeAnimation]);
  return highlightedMessageId;
}

/// Sets [ready] once the list has laid out the reply at [targetIndex].
void _useHighlightTargetLayout(
  BuildContext context,
  ItemPositionsListener listener,
  int? targetIndex,
  ValueNotifier<bool> ready,
) {
  useEffect(() {
    if (targetIndex == null || ready.value) return null;
    var completionScheduled = false;
    void markReadyAfterTargetLayout() {
      if (completionScheduled ||
          !listener.itemPositions.value.any(
            (position) => position.index == targetIndex,
          )) {
        return;
      }
      completionScheduled = true;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (context.mounted) ready.value = true;
      });
    }

    listener.itemPositions.addListener(markReadyAfterTargetLayout);
    markReadyAfterTargetLayout();
    return () =>
        listener.itemPositions.removeListener(markReadyAfterTargetLayout);
  }, [targetIndex, ready.value]);
}

/// The longest a thread keeps its list out of sight while it opens.
const _openHiddenCap = Duration(milliseconds: 1200);

/// Keeps the list out of sight about a second at most while it opens or
/// positions, the read state wait included: the replies at hand then show at
/// the newest, and the list is never hidden again. Returns whether that
/// happened; a window found later then moves there once, only if the reader
/// has not moved meanwhile (docs/message-sync.md#opening-at-a-position).
ValueNotifier<bool> _useOpenHiddenCap(
  BuildContext context,
  ValueNotifier<_ThreadOpenPhase> openPhase,
) {
  final capped = useState(false);
  useEffect(() {
    if (openPhase.value != _ThreadOpenPhase.opening) return null;
    final timer = Timer(_openHiddenCap, () {
      if (!context.mounted) return;
      final phase = openPhase.value;
      if (phase == _ThreadOpenPhase.positioning) {
        // Positioning finishes in sight.
        capped.value = true;
        return;
      }
      if (phase != _ThreadOpenPhase.opening) return;
      capped.value = true;
      openPhase.value = _ThreadOpenPhase.none;
    });
    return timer.cancel;
  }, const []);
  return capped;
}

/// Lets a thread that gave up on its linked reply for want of its time open
/// there when [createdAt] comes: past the cap, so the move happens in sight
/// and only if the reader has not moved. Returns the mark to set on giving up.
ObjectRef<bool> _useLateLinkedReply(
  int? createdAt, {
  required ObjectRef<bool> didStartOpening,
  required ValueNotifier<bool> capped,
}) {
  final gaveUp = useRef(false);
  useEffect(() {
    if (createdAt == null || !gaveUp.value) return null;
    gaveUp.value = false;
    didStartOpening.value = false;
    capped.value = true;
    return null;
  }, [createdAt]);
  return gaveUp;
}

/// The top row while the root is still on the way: the thread's name.
class _ThreadRootPendingRow extends StatelessWidget {
  final String label;
  final Key rowKey;

  const _ThreadRootPendingRow({
    required this.label,
    this.rowKey = const ValueKey('thread-root-pending'),
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      key: rowKey,
      padding: const EdgeInsets.symmetric(vertical: Grid.xs),
      child: Text(
        label,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: context.textTheme.labelMedium?.copyWith(
          color: context.colors.onSurfaceVariant,
          fontWeight: FontWeight.w600,
        ),
      ),
    );
  }
}
