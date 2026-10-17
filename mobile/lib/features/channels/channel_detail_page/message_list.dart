part of '../channel_detail_page.dart';

class _MessageList extends HookConsumerWidget {
  final List<MainTimelineEntry> entries;
  final List<TimelineMessage> allMessages;
  final String? initialMessageId;
  final String? initialThreadRootId;
  final InitialThreadRouteBehavior initialThreadRouteBehavior;
  final bool highlightInitialMessage;
  final Set<String> initialOrdinaryUnreadMessageIds;
  final String? initialOldestOrdinaryUnreadMessageId;
  final Set<String> initialForcedUnreadMessageIds;
  final bool hasInitialUnread;

  /// The message the window was opened at, if it was not the newest row.
  final _OpenedAt? openedAt;

  /// Whether the list is settling its first position at [openedAt] while
  /// still out of sight; [onPositioned] reports when it has.
  final bool isPositioning;
  final VoidCallback? onPositioned;

  /// Where this room's reading position is kept; null keeps none.
  final RoomPositionStore? roomPositions;
  final String channelId;
  final String? currentPubkey;
  final bool isMember;
  final bool isArchived;
  final double appBarTitleContentHeight;
  final double composerBottomInset;
  final FocusNode? composerFocusNode;
  final VoidCallback? restoreComposerFocus;

  /// The main composer that "Quote" fills; null when the channel can't send.
  final ComposerQuoteScope? quoteScope;

  const _MessageList({
    required this.entries,
    required this.allMessages,
    required this.initialMessageId,
    required this.initialThreadRootId,
    required this.initialThreadRouteBehavior,
    this.highlightInitialMessage = false,
    required this.initialOrdinaryUnreadMessageIds,
    required this.initialOldestOrdinaryUnreadMessageId,
    required this.initialForcedUnreadMessageIds,
    required this.hasInitialUnread,
    this.openedAt,
    this.isPositioning = false,
    this.onPositioned,
    this.roomPositions,
    required this.channelId,
    required this.currentPubkey,
    required this.isMember,
    required this.isArchived,
    required this.appBarTitleContentHeight,
    required this.composerBottomInset,
    this.composerFocusNode,
    this.restoreComposerFocus,
    this.quoteScope,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final appView = View.of(context);
    final localSendAnimations = ref.watch(
      localMessageSendAnimationProvider(channelId),
    );
    final itemScrollController = useMemoized(ItemScrollController.new);
    final itemPositionsListener = useMemoized(ItemPositionsListener.create);
    final stickyDateHeaderState = useValueNotifier(
      StickyDateHeaderState.hidden,
    );
    final stickyDayTimestamp = useValueNotifier<int?>(null);
    final timelineViewportHeight = useRef(MediaQuery.sizeOf(context).height);
    final isLoadingOlder = useState(false);
    final isAtLatest = useState(true);
    final settledImeBottomInset = useState(
      usesFixedAndroidImeViewport
          ? appView.viewInsets.bottom / appView.devicePixelRatio
          : 0.0,
    );
    final isJumpToLatestVisible = useState(false);
    final hasUserScrolled = useState(false);
    final distanceFromLatest = useRef(0.0);
    final hasUnseenLatestEntry = useRef(false);
    final followsLatest = useState(
      openedAt == null &&
          initialMessageId == null &&
          initialThreadRootId == null,
    );
    final isAutoScrolling = useRef(false);
    final isUserScrolling = useRef(false);
    final notifier = ref.read(channelMessagesProvider(channelId).notifier);
    final windowGeneration = notifier.windowGeneration;
    final placement = useRef<_ListPlacement?>(null);
    final renderedEntries = useRef<List<MainTimelineEntry>?>(null);
    final holdsNewerRows = useRef(false);
    final awaitingPlacementLayout = useRef(false);
    final heldRowsRelease = useState(0);
    final previousPlacement = placement.value;
    final replacedWindow =
        previousPlacement != null &&
        previousPlacement.generation != windowGeneration;
    var entries = this.entries;
    var displayEntries = groupMembershipTimelineEntries(entries);
    if (previousPlacement == null || replacedWindow) {
      // A new window (opened at a message, or back at the newest rows) gets a
      // new list drawn at its place from the first frame.
      final targetIndex = isPositioning && openedAt != null
          ? _openedAtIndex(displayEntries, openedAt!)
          : null;
      final topLine =
          1 -
          frostedAppBarHeight(
                context,
                titleContentHeight: appBarTitleContentHeight,
              ) /
              timelineViewportHeight.value;
      placement.value = _ListPlacement(
        generation: windowGeneration,
        epoch: (previousPlacement?.epoch ?? -1) + 1,
        index: targetIndex ?? 0,
        alignment: targetIndex == null || targetIndex == 0
            ? 0
            : openedAt!.edge ??
                  (openedAt!.isUnread ? topLine - 0.15 : 0.35),
      );
      holdsNewerRows.value = false;
      awaitingPlacementLayout.value = true;
    } else if (notifier.isInMiddleWindow && renderedEntries.value != null) {
      // Rows paged in (or arriving) below a window opened at a message would
      // move every visible row. Redraw the list around the row the reader is
      // looking at instead, once nothing is scrolling.
      final rendered = groupMembershipTimelineEntries(renderedEntries.value!);
      final previousNewestIndex = rendered.isEmpty
          ? null
          : _reversedGroupIndex(displayEntries, rendered.last.first.message.id);
      final followsTail = followsLatest.value && !hasUserScrolled.value;
      if (previousNewestIndex != null &&
          previousNewestIndex > 0 &&
          !followsTail) {
        if (isPositioning ||
            isUserScrolling.value ||
            isAutoScrolling.value ||
            awaitingPlacementLayout.value) {
          holdsNewerRows.value = true;
          entries = renderedEntries.value!;
          displayEntries = rendered;
        } else {
          holdsNewerRows.value = false;
          final reanchored = _reanchoredPlacement(
            placement.value!,
            itemPositionsListener.itemPositions.value,
            rendered,
            displayEntries,
            viewportHeight: timelineViewportHeight.value,
          );
          if (reanchored != null) {
            placement.value = reanchored;
            awaitingPlacementLayout.value = true;
          }
        }
      }
    }
    renderedEntries.value = entries;
    final displayEntriesRef = useRef(displayEntries)..value = displayEntries;
    final isPositioningRef = useRef(isPositioning)..value = isPositioning;
    final isLoadingNewer = useRef(false);
    final isJumpingToLatestWindow = useRef(false);
    final heldLocalSendIds = useRef(<String>{});
    final latestEffectGeneration = useRef(windowGeneration);
    // The "New messages" divider goes above the first unread message.
    final newMessagesIndex = openedAt?.isUnread == true
        ? _reversedGroupIndex(displayEntries, openedAt!.messageId)
        : null;
    final latestNavigationRequest = useState(0);
    final latestNavigationTargetId = useRef<String?>(null);
    final latestRealignmentQueued = useRef(false);
    final latestEntryId = entries.isEmpty ? null : entries.last.message.id;
    final previousLatestEntryId = useRef<String?>(null);
    final observedLocalSendIds = useRef(localSendAnimations.keys.toSet());
    final didOpenInitialThread = useRef(false);
    final didJumpToInitialMessage = useRef(false);
    final landingHighlightId = useState<String?>(null);
    useEffect(() {
      if (landingHighlightId.value == null) return null;
      final timer = Timer(
        const Duration(seconds: 3),
        () => landingHighlightId.value = null,
      );
      return timer.cancel;
    }, [landingHighlightId.value]);
    final isUnreadNavigationDismissed = useState(false);
    final detachedWhileUnreadShown = useRef(false);
    final oldestUnreadMessageId = useState<String?>(null);
    final unreadBoundaryLoadFailed = useState(false);
    final unreadBoundaryFetchCount = useRef(0);
    final hasUnreadDeepLink =
        initialMessageId != null || initialThreadRootId != null;
    final dayTimestampByReversedIndex = <int, int>{};
    final dayStartByReversedIndex = <int, int>{};
    final dayHeaderTimestampByReversedIndex = <int, int>{};
    final settledImeLift = usesFixedAndroidImeViewport
        ? (settledImeBottomInset.value -
                  MediaQuery.viewPaddingOf(context).bottom)
              .clamp(0.0, double.infinity)
              .toDouble()
        : settledImeBottomInset.value;
    final timelineBottomInset =
        composerBottomInset + (followsLatest.value ? settledImeLift : 0);
    final navigationBottomInset = composerBottomInset + settledImeLift;
    var currentDayTimestamp =
        displayEntries.firstOrNull?.first.message.createdAt;
    var currentDayStartIndex = displayEntries.isEmpty
        ? -1
        : displayEntries.length - 1;
    for (
      var chronologicalIndex = 0;
      chronologicalIndex < displayEntries.length;
      chronologicalIndex += 1
    ) {
      final message = displayEntries[chronologicalIndex].first.message;
      final previousMessage = chronologicalIndex > 0
          ? displayEntries[chronologicalIndex - 1].last.message
          : null;
      final startsDay =
          previousMessage == null ||
          !isSameDay(previousMessage.createdAt, message.createdAt);
      final reversedIndex = displayEntries.length - 1 - chronologicalIndex;
      if (startsDay) {
        currentDayTimestamp = message.createdAt;
        currentDayStartIndex = reversedIndex;
        dayHeaderTimestampByReversedIndex[reversedIndex] = message.createdAt;
      }
      final dayTimestamp = currentDayTimestamp;
      if (dayTimestamp != null) {
        dayTimestampByReversedIndex[reversedIndex] = dayTimestamp;
        dayStartByReversedIndex[reversedIndex] = currentDayStartIndex;
      }
    }

    useEffect(
      () {
        if (!hasInitialUnread ||
            hasUnreadDeepLink ||
            oldestUnreadMessageId.value != null ||
            unreadBoundaryLoadFailed.value ||
            entries.isEmpty ||
            // Wait for the relay page: cached rows omit unread arrivals.
            notifier.isShowingCachedHead) {
          return null;
        }

        final hasLoadedOrdinaryTarget =
            initialOldestOrdinaryUnreadMessageId != null &&
            entries.any(
              (entry) =>
                  entry.message.id == initialOldestOrdinaryUnreadMessageId,
            );
        final hasLoadedForcedTarget = entries.any(
          (entry) => initialForcedUnreadMessageIds.contains(entry.message.id),
        );
        final hasKnownTarget =
            initialOldestOrdinaryUnreadMessageId != null ||
            initialForcedUnreadMessageIds.isNotEmpty;
        final hasLoadedFetchTarget =
            initialOldestOrdinaryUnreadMessageId != null
            ? hasLoadedOrdinaryTarget
            : hasLoadedForcedTarget;
        final canFetchTarget =
            hasKnownTarget &&
            !hasLoadedFetchTarget &&
            !notifier.reachedOldest &&
            unreadBoundaryFetchCount.value < 4;
        if (canFetchTarget) {
          unreadBoundaryFetchCount.value += 1;
          var cancelled = false;
          unawaited(
            Future<void>(() async {
              final loaded = await notifier.fetchOlder();
              if (!cancelled && !loaded && !notifier.reachedOldest) {
                unreadBoundaryLoadFailed.value = true;
              }
            }),
          );
          return () => cancelled = true;
        }

        if (hasKnownTarget &&
            !hasLoadedFetchTarget &&
            !notifier.reachedOldest) {
          unreadBoundaryLoadFailed.value = true;
        }

        final ordinaryUnread = entries
            .where(
              (entry) =>
                  initialOrdinaryUnreadMessageIds.contains(entry.message.id),
            )
            .map((entry) => entry.message)
            .firstOrNull;
        final forcedUnread = entries
            .where(
              (entry) =>
                  initialForcedUnreadMessageIds.contains(entry.message.id),
            )
            .map((entry) => entry.message)
            .firstOrNull;
        final candidates = [ordinaryUnread, forcedUnread].nonNulls.toList()
          ..sort((a, b) => a.createdAt.compareTo(b.createdAt));
        oldestUnreadMessageId.value = candidates.firstOrNull?.id;
        return null;
      },
      [
        hasInitialUnread,
        hasUnreadDeepLink,
        initialOrdinaryUnreadMessageIds,
        initialOldestOrdinaryUnreadMessageId,
        initialForcedUnreadMessageIds,
        entries.length,
        notifier.reachedOldest,
        notifier.isShowingCachedHead,
        unreadBoundaryLoadFailed.value,
      ],
    );

    final showUnreadNavigation =
        !isUnreadNavigationDismissed.value &&
        oldestUnreadMessageId.value != null;

    int? reversedIndexOf(String? messageId) {
      if (messageId == null) return null;
      final chronologicalIndex = displayEntries.indexWhere(
        (group) => group.any((entry) => entry.message.id == messageId),
      );
      return chronologicalIndex < 0
          ? null
          : displayEntries.length - 1 - chronologicalIndex;
    }

    double latestAlignment() {
      final viewportHeight = timelineViewportHeight.value;
      return viewportHeight > 0
          ? (timelineBottomInset / viewportHeight).clamp(0.0, 1.0).toDouble()
          : 0.0;
    }

    void updateStickyDateHeader(Iterable<ItemPosition> rawPositions) {
      void setStickyDateHeader(
        StickyDateHeaderState state, {
        int? activeDayTimestamp,
      }) {
        stickyDateHeaderState.value = state;
        stickyDayTimestamp.value = activeDayTimestamp;
      }

      final viewportHeight = timelineViewportHeight.value;
      if (viewportHeight <= 0 || displayEntries.isEmpty) {
        setStickyDateHeader(StickyDateHeaderState.hidden);
        return;
      }

      final positions = rawPositions
          .where(
            (position) =>
                position.index < displayEntries.length &&
                position.itemLeadingEdge < 1 &&
                position.itemTrailingEdge > 0,
          )
          .toList();
      if (positions.isEmpty) {
        if (!isLoadingOlder.value) {
          setStickyDateHeader(StickyDateHeaderState.hidden);
        }
        return;
      }

      final stickyTop =
          frostedAppBarHeight(
            context,
            titleContentHeight: appBarTitleContentHeight,
          ) +
          Grid.twelve;
      double physicalTop(ItemPosition position) =>
          viewportHeight * (1 - position.itemTrailingEdge);
      double physicalBottom(ItemPosition position) =>
          viewportHeight * (1 - position.itemLeadingEdge);

      final positionAtStickyTop = positions
          .where(
            (position) =>
                physicalTop(position) <= stickyTop &&
                physicalBottom(position) > stickyTop,
          )
          .firstOrNull;
      if (positionAtStickyTop == null) {
        if (!isLoadingOlder.value) {
          setStickyDateHeader(StickyDateHeaderState.hidden);
        }
        return;
      }

      final activeDayTimestamp =
          dayTimestampByReversedIndex[positionAtStickyTop.index];
      final activeDayStartIndex =
          dayStartByReversedIndex[positionAtStickyTop.index];
      if (activeDayTimestamp == null || activeDayStartIndex == null) {
        setStickyDateHeader(StickyDateHeaderState.hidden);
        return;
      }

      final activeHeaderPosition = positions
          .where((position) => position.index == activeDayStartIndex)
          .firstOrNull;
      final oldestVisibleIndex = positions
          .map((position) => position.index)
          .reduce((a, b) => a > b ? a : b);
      final activeHeaderHasCrossed = activeHeaderPosition != null
          ? physicalTop(activeHeaderPosition) <= stickyTop
          : activeDayStartIndex > oldestVisibleIndex;
      if (!activeHeaderHasCrossed) {
        setStickyDateHeader(StickyDateHeaderState.hidden);
        return;
      }

      double? nextHeaderTop;
      for (final position in positions) {
        if (!dayHeaderTimestampByReversedIndex.containsKey(position.index) ||
            position.index >= activeDayStartIndex) {
          continue;
        }
        final top = physicalTop(position);
        if (top <= stickyTop ||
            (nextHeaderTop != null && top >= nextHeaderTop)) {
          continue;
        }
        nextHeaderTop = top;
      }

      final stickyHeaderHeight = StickyDateHeader.heightOf(context);
      final rawTranslateY = nextHeaderTop == null
          ? 0.0
          : min(0.0, nextHeaderTop - stickyTop - stickyHeaderHeight - 5);
      final translateY = rawTranslateY
          .clamp(-(stickyHeaderHeight + 5), 0.0)
          .toDouble();
      setStickyDateHeader(
        StickyDateHeaderState(
          label: formatDayHeading(activeDayTimestamp),
          translateY: (translateY * 2).round() / 2,
        ),
        activeDayTimestamp: activeDayTimestamp,
      );
    }

    bool latestIsAtBoundary() {
      // Rows newer than the loaded ones are still on the relay.
      if (notifier.hasNewer) return false;
      // In this reversed list, item 0's leading edge is the visible bottom
      // boundary above the composer. Being merely visible is not enough: a
      // user who has pulled a tall newest row away from that boundary must not
      // snap back on live updates.
      final boundary = latestAlignment();
      return itemPositionsListener.itemPositions.value.any(
        (position) =>
            position.index == 0 &&
            (position.itemLeadingEdge - boundary).abs() < 0.01,
      );
    }

    // At the newest row, a list drawn around an older row is redrawn around
    // the newest one (no visible change), so rows arriving below it follow.
    void anchorAtLatestRow() {
      if (notifier.isInMiddleWindow &&
          itemScrollController.isAttached &&
          latestIsAtBoundary()) {
        itemScrollController.jumpTo(index: 0);
      }
    }

    // Rows held back while the list moved come in once it rests.
    void releaseHeldRows() {
      if (!holdsNewerRows.value) return;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (context.mounted && holdsNewerRows.value) {
          heldRowsRelease.value += 1;
        }
      });
    }

    // A window opened at a message reaches the newest rows by replacing it.
    void jumpToLatestWindow() {
      if (isJumpingToLatestWindow.value) return;
      isJumpingToLatestWindow.value = true;
      notifier.jumpToLatest().whenComplete(
        () => isJumpingToLatestWindow.value = false,
      );
    }

    Future<void> performLatestNavigation() async {
      if (!context.mounted || !itemScrollController.isAttached) {
        isAutoScrolling.value = false;
        return;
      }
      try {
        final targetIndex =
            reversedIndexOf(latestNavigationTargetId.value) ?? 0;
        await itemScrollController.scrollTo(
          index: targetIndex,
          alignment: latestAlignment(),
          duration: jumpToLatestScrollDuration,
          curve: jumpToLatestScrollCurve,
        );
        if (context.mounted && !hasUserScrolled.value) {
          isAtLatest.value = targetIndex == 0;
          isJumpToLatestVisible.value = false;
          if (targetIndex == 0) anchorAtLatestRow();
        }
      } finally {
        isAutoScrolling.value = false;
        releaseHeldRows();
      }
    }

    void scrollToLatest({String? targetMessageId}) {
      if (!itemScrollController.isAttached || isAutoScrolling.value) return;
      isAutoScrolling.value = true;
      followsLatest.value = true;
      hasUserScrolled.value = false;
      hasUnseenLatestEntry.value = false;
      latestNavigationTargetId.value = targetMessageId;
      latestNavigationRequest.value += 1;
    }

    useEffect(() {
      if (latestNavigationRequest.value == 0) return null;
      var cancelled = false;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (cancelled) return;
        unawaited(performLatestNavigation());
      });
      return () => cancelled = true;
    }, [latestNavigationRequest.value]);

    Future<void> scrollToOlderEntry(int targetIndex) async {
      followsLatest.value = false;
      hasUserScrolled.value = false;
      isAtLatest.value = false;
      isAutoScrolling.value = true;
      try {
        await itemScrollController.scrollTo(
          index: targetIndex,
          alignment: 0.35,
          duration: const Duration(milliseconds: 220),
          curve: Curves.easeOutCubic,
        );
      } finally {
        isAutoScrolling.value = false;
        releaseHeldRows();
      }
    }

    Future<void> scrollToOldestUnread() async {
      final targetIndex = reversedIndexOf(oldestUnreadMessageId.value);
      if (targetIndex == null ||
          !itemScrollController.isAttached ||
          isAutoScrolling.value) {
        return;
      }
      isUnreadNavigationDismissed.value = true;
      await scrollToOlderEntry(targetIndex);
    }

    // A quote of a row this list already shows scrolls in place instead of
    // stacking a second copy of this channel through the deep link.
    bool jumpToQuotedMessage(String messageId) {
      final targetIndex = reversedIndexOf(messageId);
      if (targetIndex == null || !itemScrollController.isAttached) {
        return false;
      }
      if (!isAutoScrolling.value) unawaited(scrollToOlderEntry(targetIndex));
      return true;
    }

    void updateJumpToLatestVisibility(
      Iterable<ItemPosition> positions, {
      double? viewportDimension,
    }) {
      final latestIsVisible = positions.any(
        (position) =>
            position.index == 0 &&
            position.itemLeadingEdge < 1 &&
            position.itemTrailingEdge > latestAlignment(),
      );
      final viewportHeight = viewportDimension ?? timelineViewportHeight.value;
      final visiblePageHeight = max(
        0.0,
        viewportHeight -
            frostedAppBarHeight(
              context,
              titleContentHeight: appBarTitleContentHeight,
            ) -
            composerBottomInset,
      );
      final hidesJumpToLatest = shouldHideChannelJumpToLatest(
        isAtLatest: latestIsAtBoundary(),
        followsLatest: followsLatest.value,
        userHasDetached: hasUserScrolled.value,
      );
      final shouldShow =
          !hidesJumpToLatest &&
          (hasUnseenLatestEntry.value ||
              !latestIsVisible ||
              distanceFromLatest.value > visiblePageHeight);
      if (isJumpToLatestVisible.value != shouldShow) {
        isJumpToLatestVisible.value = shouldShow;
      }
    }

    void realignLatestAfterLayoutChange() {
      if (latestRealignmentQueued.value ||
          isAutoScrolling.value ||
          !followsLatest.value ||
          hasUserScrolled.value) {
        return;
      }
      latestRealignmentQueued.value = true;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        latestRealignmentQueued.value = false;
        if (!context.mounted ||
            !itemScrollController.isAttached ||
            isAutoScrolling.value ||
            !followsLatest.value ||
            hasUserScrolled.value ||
            latestIsAtBoundary()) {
          return;
        }
        // A dock or keyboard resize is a layout correction, not a navigation
        // action. Keeping it instant avoids restarting a smooth scroll for
        // every position report while the viewport settles.
        itemScrollController.jumpTo(index: 0);
      });
    }

    useEffect(
      () {
        void onPositionsChanged() {
          final positions = itemPositionsListener.itemPositions.value;
          if (positions.isEmpty) return;
          if (awaitingPlacementLayout.value) {
            awaitingPlacementLayout.value = false;
            releaseHeldRows();
          }
          updateStickyDateHeader(positions);
          updateJumpToLatestVisibility(positions);
          final nextIsAtLatest = latestIsAtBoundary();
          if (showUnreadNavigation &&
              nextIsAtLatest &&
              detachedWhileUnreadShown.value) {
            isUnreadNavigationDismissed.value = true;
          }
          if (nextIsAtLatest) {
            hasUnseenLatestEntry.value = false;
            if (!isAtLatest.value) isAtLatest.value = true;
            if (isJumpToLatestVisible.value) {
              isJumpToLatestVisible.value = false;
            }
          } else if (!followsLatest.value && isAtLatest.value) {
            isAtLatest.value = false;
          }

          if (notifier.hasNewer &&
              !isLoadingNewer.value &&
              !isPositioningRef.value) {
            final newestVisible = positions
                .map((position) => position.index)
                .reduce(min);
            if (newestVisible <= 2) {
              isLoadingNewer.value = true;
              notifier.fetchNewer().whenComplete(
                () => isLoadingNewer.value = false,
              );
            }
          }

          final oldestVisible = positions
              .map((position) => position.index)
              .reduce((a, b) => a > b ? a : b);
          if (!hasUserScrolled.value ||
              oldestVisible < displayEntries.length - 3 ||
              isLoadingOlder.value) {
            return;
          }
          if (notifier.reachedOldest) return;
          isLoadingOlder.value = true;
          notifier.fetchOlder().whenComplete(
            () => isLoadingOlder.value = false,
          );
        }

        var disposed = false;
        itemPositionsListener.itemPositions.addListener(onPositionsChanged);
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (!disposed && context.mounted) onPositionsChanged();
        });
        return () {
          disposed = true;
          itemPositionsListener.itemPositions.removeListener(
            onPositionsChanged,
          );
        };
      },
      [
        channelId,
        entries.length,
        itemPositionsListener,
        appBarTitleContentHeight,
        composerBottomInset,
      ],
    );

    useEffect(() {
      stickyDateHeaderState.value = StickyDateHeaderState.hidden;
      stickyDayTimestamp.value = null;
      return null;
    }, [channelId]);

    // A replaced window is drawn afresh, at its newest row unless more rows
    // remain below it.
    final seenWindowGeneration = useRef(windowGeneration);
    useEffect(() {
      if (seenWindowGeneration.value == windowGeneration) return null;
      seenWindowGeneration.value = windowGeneration;
      final atNewest = !notifier.hasNewer;
      hasUserScrolled.value = false;
      followsLatest.value = atNewest;
      isAtLatest.value = atNewest;
      hasUnseenLatestEntry.value = false;
      isJumpToLatestVisible.value = false;
      return null;
    }, [windowGeneration]);

    // While out of sight, settle the window opened at a message so it fills
    // the viewport: the newest row is not lifted off the composer, the oldest
    // not pulled under the app bar, and the first unread sits at the top.
    useEffect(() {
      if (!isPositioning) return null;
      final target = openedAt;
      var done = false;
      var steps = 0;
      var index = placement.value?.index ?? 0;
      var alignment = placement.value?.alignment ?? 0.0;
      Timer? fallback;

      void finish() {
        if (done) return;
        done = true;
        fallback?.cancel();
        final atNewest = index == 0 && !notifier.hasNewer;
        followsLatest.value = atNewest;
        isAtLatest.value = atNewest;
        hasUserScrolled.value = false;
        if (target != null && !target.isUnread && highlightInitialMessage) {
          landingHighlightId.value = initialMessageId;
        }
        onPositioned?.call();
      }

      void jump(int nextIndex, double nextAlignment) {
        index = nextIndex;
        alignment = nextAlignment;
        itemScrollController.jumpTo(index: nextIndex, alignment: nextAlignment);
      }

      void step() {
        if (done || !context.mounted) return;
        final count = displayEntriesRef.value.length;
        if (count == 0) return finish();
        final positions = itemPositionsListener.itemPositions.value
            .where((position) => position.index < count)
            .toList();
        if (positions.isEmpty || !itemScrollController.isAttached) return;
        if (index == 0 || steps >= 4) return finish();
        steps += 1;
        final viewportHeight = timelineViewportHeight.value;
        if (viewportHeight <= 0) return finish();
        final tolerance = 1.5 / viewportHeight;
        final topLine =
            1 -
            frostedAppBarHeight(
                  context,
                  titleContentHeight: appBarTitleContentHeight,
                ) /
                viewportHeight;
        ItemPosition? positionOf(int itemIndex) => positions
            .where((position) => position.index == itemIndex)
            .firstOrNull;

        final newest = positionOf(0);
        if (newest != null &&
            newest.itemLeadingEdge > latestAlignment() + tolerance) {
          return jump(0, 0);
        }
        final anchor = positionOf(index);
        if (target != null && target.isUnread && anchor != null) {
          final shift = topLine - anchor.itemTrailingEdge;
          if (shift.abs() > tolerance) return jump(index, alignment + shift);
        }
        final oldest = positionOf(count - 1);
        if (oldest != null && oldest.itemTrailingEdge < topLine - tolerance) {
          return jump(index, alignment + topLine - oldest.itemTrailingEdge);
        }
        finish();
      }

      void onPositionsChanged() => scheduleMicrotask(step);

      itemPositionsListener.itemPositions.addListener(onPositionsChanged);
      if (displayEntriesRef.value.isEmpty) {
        scheduleMicrotask(finish);
      } else {
        // The list never reports a position when nothing lays out.
        fallback = Timer(const Duration(seconds: 1), finish);
      }
      return () {
        done = true;
        fallback?.cancel();
        itemPositionsListener.itemPositions.removeListener(onPositionsChanged);
      };
    }, [isPositioning]);

    // Composer size changes and keyboard metrics changes arrive in separate
    // layout passes. Preserve the latest-message anchor for both, but only
    // while the user has not deliberately left the tail.
    useEffect(() {
      realignLatestAfterLayoutChange();
      return null;
    }, [timelineBottomInset]);

    useEffect(() {
      final observer = ImeMetricsSettleObserver(
        onMetricsSettled: () {
          if (!usesFixedAndroidImeViewport) {
            realignLatestAfterLayoutChange();
            return;
          }
          final nextInset =
              appView.viewInsets.bottom / appView.devicePixelRatio;
          if ((settledImeBottomInset.value - nextInset).abs() >= 0.5) {
            settledImeBottomInset.value = nextInset;
          }
        },
      );
      WidgetsBinding.instance.addObserver(observer);
      return () {
        WidgetsBinding.instance.removeObserver(observer);
        observer.dispose();
      };
    }, [appView, itemScrollController]);

    useEffect(() {
      if (initialThreadRootId == null || didOpenInitialThread.value) {
        return null;
      }
      final threadHead = allMessages
          .where((message) => message.id == initialThreadRootId)
          .firstOrNull;
      if (threadHead == null) return null;
      didOpenInitialThread.value = true;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!context.mounted || ModalRoute.of(context)?.isCurrent != true) {
          return;
        }
        final route = MaterialPageRoute<void>(
          builder: (_) => ThreadDetailPage(
            threadHead: threadHead,
            allMessages: allMessages,
            channelId: channelId,
            currentPubkey: currentPubkey,
            isMember: isMember,
            isArchived: isArchived,
            initialMessageId: initialMessageId,
          ),
        );
        final navigator = Navigator.of(context);
        switch (initialThreadRouteBehavior) {
          case InitialThreadRouteBehavior.push:
            navigator.push(route);
          case InitialThreadRouteBehavior.replaceCurrentRoute:
            navigator.pushReplacement(route);
        }
      });
      return null;
    }, [initialThreadRootId, allMessages, initialThreadRouteBehavior]);

    useEffect(() {
      final targetIndex = reversedIndexOf(initialMessageId);
      if (openedAt != null ||
          initialThreadRootId != null ||
          targetIndex == null ||
          didJumpToInitialMessage.value) {
        return null;
      }
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!context.mounted || !itemScrollController.isAttached) return;
        didJumpToInitialMessage.value = true;
        followsLatest.value = false;
        hasUserScrolled.value = false;
        isAtLatest.value = false;
        itemScrollController.jumpTo(index: targetIndex, alignment: 0.35);
        if (highlightInitialMessage) {
          landingHighlightId.value = initialMessageId;
        }
      });
      return null;
    }, [initialMessageId, initialThreadRootId, entries.length, openedAt]);

    useEffect(() {
      final previous = previousLatestEntryId.value;
      previousLatestEntryId.value = latestEntryId;
      final entryIds = entries.map((entry) => entry.message.id).toSet();
      final newlyInsertedLocalSendIds = localSendAnimations.keys
          .where(
            (eventId) =>
                !observedLocalSendIds.value.contains(eventId) &&
                entryIds.contains(eventId),
          )
          .toList();
      observedLocalSendIds.value
        ..removeWhere((eventId) => !localSendAnimations.containsKey(eventId))
        ..addAll(newlyInsertedLocalSendIds);
      final windowReplaced = latestEffectGeneration.value != windowGeneration;
      latestEffectGeneration.value = windowGeneration;
      // A replaced window is already drawn at its place.
      if (windowReplaced) return null;
      heldLocalSendIds.value.removeWhere(
        (eventId) => !localSendAnimations.containsKey(eventId),
      );
      if (notifier.hasNewer) {
        // A message sent here waits below rows not loaded yet; go to it.
        final heldSend = localSendAnimations.keys.any(
          (eventId) =>
              !observedLocalSendIds.value.contains(eventId) &&
              !entryIds.contains(eventId) &&
              heldLocalSendIds.value.add(eventId),
        );
        if (heldSend) jumpToLatestWindow();
        return null;
      }
      final localSendId = newlyInsertedLocalSendIds.firstOrNull;
      final latestEntryChanged =
          previous != null &&
          latestEntryId != null &&
          previous != latestEntryId;
      if (!latestEntryChanged && localSendId == null) {
        return null;
      }
      if (localSendId != null) {
        followsLatest.value = true;
        hasUserScrolled.value = false;
      }
      if (!followsLatest.value || hasUserScrolled.value) {
        hasUnseenLatestEntry.value = true;
      }
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!context.mounted) return;
        if (followsLatest.value && !hasUserScrolled.value) {
          scrollToLatest(targetMessageId: localSendId);
          return;
        }
        final positions = itemPositionsListener.itemPositions.value;
        if (positions.isNotEmpty) {
          if (latestIsAtBoundary()) {
            hasUnseenLatestEntry.value = false;
          }
          updateJumpToLatestVisibility(positions);
        }
      });
      return null;
    }, [latestEntryId, localSendAnimations, windowGeneration]);

    final saveRoomPosition = useRoomPositionSaving(
      store: roomPositions,
      roomKey: channelId,
      read: () {
        if (isPositioningRef.value) return null;
        final positions = itemPositionsListener.itemPositions.value;
        if (positions.isEmpty || displayEntriesRef.value.isEmpty) return null;
        if (latestIsAtBoundary()) return const RoomPositionReading.atLatest();
        return _readingPosition(
          positions,
          displayEntriesRef.value,
          bottom: latestAlignment(),
        );
      },
    );

    if (entries.isEmpty) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              LucideIcons.messageSquare,
              size: Grid.xl,
              color: context.colors.onSurfaceVariant,
            ),
            const SizedBox(height: Grid.xxs),
            Text(
              'No messages yet',
              style: context.textTheme.bodyLarge?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
            const SizedBox(height: Grid.half),
            Text(
              'Be the first to say something!',
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
          ],
        ),
      );
    }

    // Build channel names map once for all message bubbles.
    final channelsAsync = ref.watch(channelsProvider);
    final channelNamesMap = <String, String>{};
    channelsAsync.whenData((channels) {
      for (final ch in channels) {
        channelNamesMap[ch.name.toLowerCase()] = ch.id;
      }
    });

    return QuoteJumpScope(
      channelId: channelId,
      jumpToMessage: jumpToQuotedMessage,
      child: Stack(
        children: [
          NotificationListener<Notification>(
            onNotification: (notification) {
              if (notification is ScrollMetricsNotification &&
                  notification.depth != 0) {
                return false;
              }
              if (notification is ScrollNotification &&
                  notification.depth != 0) {
                return false;
              }
              if (notification is ScrollMetricsNotification) {
                timelineViewportHeight.value =
                    notification.metrics.viewportDimension;
                return false;
              }
              if (notification is! ScrollNotification) return false;
              if (notification is ScrollStartNotification) {
                isUserScrolling.value = true;
              } else if (notification is ScrollEndNotification) {
                isUserScrolling.value = false;
                releaseHeldRows();
                saveRoomPosition();
              }
              timelineViewportHeight.value =
                  notification.metrics.viewportDimension;
              distanceFromLatest.value = max(
                0.0,
                notification.metrics.pixels -
                    notification.metrics.minScrollExtent,
              );
              updateJumpToLatestVisibility(
                itemPositionsListener.itemPositions.value,
                viewportDimension: notification.metrics.viewportDimension,
              );
              if (notification is UserScrollNotification &&
                  notification.direction != ScrollDirection.idle) {
                hasUserScrolled.value = true;
                followsLatest.value = false;
                if (showUnreadNavigation) {
                  detachedWhileUnreadShown.value = true;
                }
              } else if (notification is ScrollEndNotification &&
                  hasUserScrolled.value) {
                WidgetsBinding.instance.addPostFrameCallback((_) {
                  if (!context.mounted || !latestIsAtBoundary()) return;
                  hasUserScrolled.value = false;
                  followsLatest.value = true;
                  if (!isAtLatest.value) isAtLatest.value = true;
                  anchorAtLatestRow();
                });
              }
              return false;
            },
            child: KeyboardDismissOnDrag(
              // A new placement draws a new list where it belongs.
              key: ValueKey(placement.value!.epoch),
              child: ScrollablePositionedList.builder(
                key: const ValueKey('channel-message-list'),
                initialScrollIndex: placement.value!.index,
                initialAlignment: placement.value!.alignment,
                itemScrollController: itemScrollController,
                itemPositionsListener: itemPositionsListener,
                reverse: true,
                padding: EdgeInsets.only(
                  left: chatListLeftInset,
                  right: chatListRightInset,
                  top: frostedAppBarHeight(
                    context,
                    titleContentHeight: appBarTitleContentHeight,
                  ),
                  bottom: timelineBottomInset,
                ),
                itemCount:
                    displayEntries.length + (isLoadingOlder.value ? 1 : 0),
                itemBuilder: (context, index) {
                  // Loading indicator at the top (last index in reversed list).
                  if (index >= displayEntries.length) {
                    return const Padding(
                      padding: EdgeInsets.symmetric(vertical: Grid.xs),
                      child: Center(
                        child: KuzzLoadingIndicator(
                          size: 24,
                          semanticLabel: 'Loading older messages',
                        ),
                      ),
                    );
                  }

                  // Reversed list: index 0 = newest (bottom of screen).
                  final chronIdx = displayEntries.length - 1 - index;
                  final entryGroup = displayEntries[chronIdx];
                  final entry = entryGroup.first;
                  final message = entry.message;

                  // Day boundary check — applies to all messages including system.
                  final prevEntry = chronIdx > 0
                      ? displayEntries[chronIdx - 1].last
                      : null;
                  final prevMessage = prevEntry?.message;
                  final showDayDivider =
                      prevMessage == null ||
                      !isSameDay(prevMessage.createdAt, message.createdAt);

                  final showAuthor =
                      !message.isSystem &&
                      (prevMessage == null ||
                          prevMessage.isSystem ||
                          showDayDivider ||
                          prevMessage.pubkey.toLowerCase() !=
                              message.pubkey.toLowerCase() ||
                          (message.createdAt - prevMessage.createdAt) > 300);

                  // The time sits under the last message of a group, so look
                  // ahead to whether the next message continues this one.
                  final nextMessage = chronIdx + 1 < displayEntries.length
                      ? displayEntries[chronIdx + 1].first.message
                      : null;
                  final showTime =
                      nextMessage == null ||
                      nextMessage.isSystem ||
                      !isSameDay(message.createdAt, nextMessage.createdAt) ||
                      nextMessage.pubkey.toLowerCase() !=
                          message.pubkey.toLowerCase() ||
                      (nextMessage.createdAt - message.createdAt) > 300;

                  return LocalMessageSendTransition(
                    key: ValueKey('channel-message-send-${message.id}'),
                    animate: isRecentLocalMessageSendAnimation(
                      localSendAnimations,
                      message.id,
                    ),
                    startOffsetFactor: showAuthor
                        ? localMessageSendTransitionAvatarStartOffset
                        : localMessageSendTransitionStartOffset,
                    child: Padding(
                      key: ValueKey('channel-message-group-${message.id}'),
                      padding: EdgeInsets.only(
                        bottom: index == 0 ? Grid.xs : 0,
                      ),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          if (showDayDivider)
                            DayDivider(
                              label: formatDayHeading(message.createdAt),
                              dayTimestamp: message.createdAt,
                              stickyDayTimestamp: stickyDayTimestamp,
                            ),
                          if (index == newMessagesIndex)
                            const NewMessagesDivider(
                              key: ValueKey('channel-new-messages-divider'),
                            ),
                          if (message.isSystem)
                            _SystemMessageRow(
                              message: message,
                              groupedMessages: entryGroup.length > 1
                                  ? entryGroup
                                        .map((entry) => entry.message)
                                        .toList()
                                  : null,
                              channelId: channelId,
                              currentPubkey: currentPubkey,
                              allMessages: allMessages,
                              isMember: isMember,
                              isArchived: isArchived,
                            )
                          else ...[
                            _LandingHighlight(
                              enabled:
                                  highlightInitialMessage &&
                                  message.id == initialMessageId,
                              highlighted:
                                  message.id == landingHighlightId.value,
                              child: _MessageBubble(
                                message: message,
                                showAuthor: showAuthor,
                                showTime: showTime,
                                hasReplies: entry.summary != null,
                                channelNames: channelNamesMap,
                                currentChannelId: channelId,
                                currentPubkey: currentPubkey,
                                allMessages: allMessages,
                                isMember: isMember,
                                isArchived: isArchived,
                                composerFocusNode: composerFocusNode,
                                restoreComposerFocus: restoreComposerFocus,
                                quoteScope: quoteScope,
                              ),
                            ),
                            if (entry.summary != null)
                              _ThreadSummaryRow(
                                summary: entry.summary!,
                                message: message,
                                allMessages: allMessages,
                                channelId: channelId,
                                currentPubkey: currentPubkey,
                                isMember: isMember,
                                isArchived: isArchived,
                              ),
                          ],
                        ],
                      ),
                    ),
                  );
                },
              ),
            ),
          ),
          if (!showUnreadNavigation)
            Positioned(
              left: 0,
              right: 0,
              top:
                  frostedAppBarHeight(
                    context,
                    titleContentHeight: appBarTitleContentHeight,
                  ) +
                  Grid.twelve,
              child: StickyDateHeader(
                key: const ValueKey('channel-sticky-date-header'),
                state: stickyDateHeaderState,
              ),
            ),
          if (showUnreadNavigation)
            Positioned(
              left: 0,
              right: 0,
              top:
                  frostedAppBarHeight(
                    context,
                    titleContentHeight: appBarTitleContentHeight,
                  ) +
                  Grid.xs,
              child: Center(
                child: IconButton.filled(
                  key: const ValueKey('channel-jump-to-oldest-unread'),
                  onPressed: scrollToOldestUnread,
                  tooltip: 'Jump to oldest unread message',
                  style: IconButton.styleFrom(
                    backgroundColor: context.colors.primaryContainer,
                    foregroundColor: context.colors.onPrimaryContainer,
                  ),
                  icon: const Icon(LucideIcons.chevronUp, size: 20),
                ),
              ),
            )
          else
            Positioned(
              left: 0,
              right: 0,
              bottom: navigationBottomInset + Grid.xs,
              child: Center(
                child: JumpToLatestSwitcher(
                  id: 'channel',
                  visible: isJumpToLatestVisible.value || notifier.hasNewer,
                  onPressed: notifier.hasNewer
                      ? jumpToLatestWindow
                      : scrollToLatest,
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// The brief tint a jumped-to message gets, like a thread's landing
/// highlight. Only the jump target is wrapped ([enabled]).
class _LandingHighlight extends StatelessWidget {
  const _LandingHighlight({
    required this.enabled,
    required this.highlighted,
    required this.child,
  });

  final bool enabled;
  final bool highlighted;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    if (!enabled) return child;
    return AnimatedContainer(
      duration: MediaQuery.disableAnimationsOf(context)
          ? Duration.zero
          : const Duration(milliseconds: 300),
      curve: Curves.easeOutCubic,
      decoration: BoxDecoration(
        color: highlighted
            ? context.colors.primary.withValues(alpha: 0.12)
            : Colors.transparent,
        borderRadius: BorderRadius.circular(Radii.md),
      ),
      child: child,
    );
  }
}

/// The list a window is drawn in: a new [epoch] draws a new list at [index]
/// and [alignment] from its first frame.
class _ListPlacement {
  const _ListPlacement({
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

/// The reversed list index of the group holding [messageId].
int? _reversedGroupIndex(
  List<List<MainTimelineEntry>> groups,
  String? messageId,
) {
  if (messageId == null) return null;
  final chronologicalIndex = groups.indexWhere(
    (group) => group.any((entry) => entry.message.id == messageId),
  );
  return chronologicalIndex < 0 ? null : groups.length - 1 - chronologicalIndex;
}

/// The reversed list index the window was opened at: its message, or the
/// oldest row not older than it.
int _openedAtIndex(List<List<MainTimelineEntry>> groups, _OpenedAt openedAt) {
  final byId = _reversedGroupIndex(groups, openedAt.messageId);
  if (byId != null) return byId;
  final chronologicalIndex = groups.indexWhere(
    (group) => group.first.message.createdAt >= openedAt.createdAt,
  );
  return chronologicalIndex < 0 ? 0 : groups.length - 1 - chronologicalIndex;
}

/// A placement that keeps the row being read where it is on screen after
/// rows were added below [rendered], now [groups].
_ListPlacement? _reanchoredPlacement(
  _ListPlacement placement,
  Iterable<ItemPosition> positions,
  List<List<MainTimelineEntry>> rendered,
  List<List<MainTimelineEntry>> groups, {
  required double viewportHeight,
}) {
  final visible = positions
      .where(
        (position) =>
            position.index < rendered.length &&
            position.itemLeadingEdge < 1 &&
            position.itemTrailingEdge > 0,
      )
      .toList();
  if (visible.isEmpty || viewportHeight <= 0) return null;
  // The newest row changes as rows join it (time label, bottom padding), so
  // the reference is the next one up when it is on screen.
  final above = visible.where((position) => position.index > 0).toList();
  final reference = above.isEmpty
      ? visible.first
      : above.reduce((a, b) => a.index < b.index ? a : b);
  final index = _reversedGroupIndex(
    groups,
    rendered[rendered.length - 1 - reference.index].first.message.id,
  );
  if (index == null) return null;
  return _ListPlacement(
    generation: placement.generation,
    epoch: placement.epoch + 1,
    index: index,
    alignment:
        reference.itemLeadingEdge +
        (reference.index == 0 && index > 0 ? Grid.xs / viewportHeight : 0),
  );
}

/// The row a reader is on: the lowest one wholly above the timeline's
/// [bottom] boundary, or the lowest one in sight.
RoomPositionReading? _readingPosition(
  Iterable<ItemPosition> positions,
  List<List<MainTimelineEntry>> groups, {
  required double bottom,
}) {
  final visible = positions
      .where(
        (position) =>
            position.index < groups.length &&
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
  final message = groups[groups.length - 1 - reading.index].first.message;
  return RoomPositionReading.at(
    id: message.id,
    createdAt: message.createdAt,
    edge: reading.itemLeadingEdge,
  );
}
