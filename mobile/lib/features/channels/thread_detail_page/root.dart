part of '../thread_detail_page.dart';

/// The page's head: the one it was pushed with, else the root as resolved
/// from its id (docs/message-sync.md#opening-a-thread-from-its-root-id),
/// else a stand-in carrying only the id while it is [resolving]. Nothing on
/// the page waits for it.
({TimelineMessage head, bool resolving, bool deleted}) _watchThreadHead(
  WidgetRef ref,
  ThreadDetailPage page,
  String? currentPubkey,
) {
  final passed = page.threadHead;
  if (passed != null) return (head: passed, resolving: false, deleted: false);
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
        TimelineMessage(id: page.headId, pubkey: '', createdAt: 0, content: ''),
    resolving: head == null,
    deleted: root.deleted,
  );
}

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

/// The top row while the root is still on the way: the thread's name.
class _ThreadRootPendingRow extends StatelessWidget {
  final String label;

  const _ThreadRootPendingRow({required this.label});

  @override
  Widget build(BuildContext context) {
    return Padding(
      key: const ValueKey('thread-root-pending'),
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
