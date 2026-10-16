import 'dart:async';
import '../../shared/read_aloud/read_aloud_message.dart';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:scrollable_positioned_list/scrollable_positioned_list.dart';

import '../../shared/devices/agent_badge_widgets.dart';
import '../../shared/mentions/agent_identity_provider.dart';
import '../../shared/relay/relay.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/avatar_image.dart';
import '../../shared/widgets/frosted_app_bar.dart';
import '../../shared/widgets/frosted_scaffold.dart';
import '../../shared/widgets/ios_glass_navigation_button.dart';
import '../../shared/widgets/keyboard_dismiss_on_drag.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/profile/user_profile.dart';
import 'android_ime_lift.dart';
import 'channel_identity_names_provider.dart';
import 'channel_link_navigation.dart';
import 'channel_messages_provider.dart';
import 'channel_preload.dart';
import 'channel_typing_provider.dart';
import 'channel_typing_indicator.dart';
import 'thread_depth.dart';
import 'thread_replies_provider.dart';
import 'thread_window.dart';
import 'thread_name_provider.dart';
import 'thread_name_editor.dart';
import 'channels_provider.dart';
import 'compose_bar.dart';
import 'composer_quote_chip.dart';
import 'composer_quote_provider.dart';
import 'composer_dock_size_reporter.dart';
import 'chat_bubble_row.dart';
import 'date_formatters.dart';
import 'day_divider.dart';
import 'ime_metrics_settle_observer.dart';
import 'laid_out_viewport.dart';
import 'jump_to_latest_button.dart';
import 'jump_to_latest_switcher.dart';
import 'local_message_send_animation_provider.dart';
import 'local_message_send_transition.dart';
import '../profile/user_profile_sheet.dart';
import '../goals/goals_preferences.dart';
import '../goals/thread_goal_chip.dart';
import 'message_actions.dart';
import 'message_action_backdrop_state.dart';
import 'message_long_press_region.dart';
import 'message_content.dart';
import 'message_quote.dart';
import 'message_quote_header.dart';
import 'reaction_row.dart';
import '../../shared/read_state/read_state_format.dart';
import '../../shared/read_state/read_state_provider.dart';
import 'send_message_provider.dart';
import 'small_avatar.dart';
import 'sticky_date_header.dart';
import 'timeline_message.dart';

part 'thread_detail_page/nested_thread_summary_row.dart';
part 'thread_detail_page/message_list.dart';
part 'thread_detail_page/sticky_date.dart';
part 'thread_detail_helpers.dart';
part 'thread_detail_page/thread_message.dart';
part 'thread_detail_page/avatar.dart';

const _landingHighlightDuration = Duration(seconds: 3);
const _landingHighlightDelay = Duration(milliseconds: 50);

/// Older pages walked to reach a linked reply before giving up (desktop: 10).
const _deepLinkMaxOlderPages = 10;
const _landingHighlightTransitionDuration = Duration(milliseconds: 300);
const _landingHighlightOpacity = 0.12;

/// Full-screen thread detail page.
///
/// Shows the thread head message, direct replies, typing indicators scoped to
/// the thread, and a compose bar for replying.
class ThreadDetailPage extends HookConsumerWidget {
  final TimelineMessage threadHead;
  final List<TimelineMessage> allMessages;
  final String channelId;
  final String? currentPubkey;
  final bool isMember;
  final bool isArchived;
  final String? initialMessageId;

  const ThreadDetailPage({
    super.key,
    required this.threadHead,
    required this.allMessages,
    required this.channelId,
    required this.currentPubkey,
    required this.isMember,
    required this.isArchived,
    this.initialMessageId,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final nameKey = (channelId: channelId, headId: threadHead.id);
    final threadName = ref.watch(threadNameProvider(nameKey)).value?.content;
    final appView = View.of(context);
    final composerDockHeight = useState(0.0);
    final composerFocusNode = useFocusNode();
    final restoreComposerFocus = useRef<VoidCallback?>(null);
    final settledImeBottomInset = useState(
      usesFixedAndroidImeViewport
          ? appView.viewInsets.bottom / appView.devicePixelRatio
          : 0.0,
    );
    useEffect(() {
      final session = ref.read(relaySessionProvider.notifier);
      return session.registerVisibleChannel(channelId);
    }, [channelId]);
    final sendMessage = ref.read(sendMessageProvider);
    final quoteScope = ComposerQuoteScope(
      channelId: channelId,
      threadHeadId: threadHead.id,
    );
    // Captured per build so a send carries the quote shown when it started.
    final pendingQuote = ref.watch(composerQuoteProvider(quoteScope));
    final localSendAnimations = ref.watch(
      localMessageSendAnimationProvider(channelId),
    );
    // Relay thread queries are keyed by the outermost root, even when this
    // page displays a nested branch. Query that root, then select this head's
    // direct children from the returned subtree below.
    final queryRootId = threadHead.rootId ?? threadHead.id;
    final repliesArgs = ThreadRepliesArgs(
      channelId: channelId,
      rootId: queryRootId,
    );
    final relayReplyState = ref.watch(threadRepliesProvider(repliesArgs));
    final repliesState = ref.watch(threadRepliesWithLocalProvider(repliesArgs));
    final relayRepliesAvailable = relayReplyState.value != null;
    // The thread query is one-shot and asks only for content kinds, so a
    // reaction, edit, or deletion that lands while the thread is open never
    // reaches it — a new pill (and its burst) only showed up after leaving and
    // re-entering, which refetched. The channel socket already receives those
    // events, so union the two sources and format once.
    final liveChannelEvents =
        ref.watch(channelMessagesProvider(channelId)).value ??
        const <NostrEvent>[];
    // Loading/error states can still carry replies from the last query.
    // whenData drops that retained value during retries.
    final replyEvents = repliesState.value;
    // Within the window's span the relay page is authoritative: a reply the
    // channel store still caches but the page dropped (deleted meanwhile), or
    // one older than the window with a gap before it, must not come back.
    final windowState = ref.watch(threadWindowProvider(repliesArgs))?.state;
    final liveThreadEvents = windowState == null
        ? liveChannelEvents
        : [
            for (final event in liveChannelEvents)
              if (event.id == threadHead.id ||
                  !isCoveredByThreadWindow(event, windowState))
                event,
          ];
    final fetchedReplies = replyEvents == null
        ? null
        : formatTimeline(
            mergeThreadEvents(replyEvents, liveThreadEvents),
            currentPubkey: currentPubkey,
          );
    final hasFetchedReplies = fetchedReplies != null;
    // A terminal query error cannot produce a more authoritative list. Keep
    // loading states provisional, but let the hydrated route snapshot drive
    // the one-shot target jump when the relay query has definitively failed.
    final canUseMessagesForInitialTarget =
        relayReplyState.value != null ||
        (relayReplyState.hasError && !relayReplyState.retrying);
    final liveDeletionHidesHead = _isDeletedBy(
      liveChannelEvents,
      threadHead.id,
    );
    // Replies preloaded from the inbox paint with the route snapshot until
    // the full scan lands; they never replace it.
    final preloadedReplies = relayRepliesAvailable
        ? const <NostrEvent>[]
        : ref.watch(preloadedThreadRepliesProvider(repliesArgs));
    final allMsgs = fetchedReplies == null || !relayRepliesAvailable
        ? _provisionalThreadMessages(
            allMessages,
            fetchedReplies ??
                formatTimeline(
                  preloadedReplies.isEmpty
                      ? liveChannelEvents
                      : mergeThreadEvents(preloadedReplies, liveChannelEvents),
                  currentPubkey: currentPubkey,
                ),
            liveChannelEvents,
          )
        : [
            // Only fall back to the pushed-route snapshot when neither source
            // carries the head, and no live deletion has suppressed it. That
            // keeps a temporarily unavailable head visible without restoring
            // a head that was deleted while this page was open.
            if (!liveDeletionHidesHead &&
                !fetchedReplies.any((message) => message.id == threadHead.id))
              threadHead,
            ...fetchedReplies,
          ];
    final routeAnimation = ModalRoute.of(context)?.animation;
    final reducedLandingHighlightMotion = MediaQuery.disableAnimationsOf(
      context,
    );
    final highlightedMessageId = useState<String?>(null);
    final initialTargetReadyForHighlight = useState(false);
    useEffect(
      () {
        final messageId = initialMessageId;
        if (messageId == null || !initialTargetReadyForHighlight.value) {
          return null;
        }
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
                    (reducedLandingHighlightMotion
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
      },
      [
        initialMessageId,
        initialTargetReadyForHighlight.value,
        reducedLandingHighlightMotion,
        routeAnimation,
      ],
    );

    // Index all messages by parentId so we can find direct children of any
    // message and compute thread summaries for nested threads.
    final childrenByParent = <String, List<TimelineMessage>>{};
    for (final msg in allMsgs) {
      final pid = msg.parentId;
      if (pid == null) continue;
      childrenByParent.putIfAbsent(pid, () => []).add(msg);
    }

    final allReplies = childrenByParent[threadHead.id] ?? const [];
    final liveHead =
        allMsgs.where((m) => m.id == threadHead.id).firstOrNull ?? threadHead;
    final itemScrollController = useMemoized(ItemScrollController.new);
    final itemPositionsListener = useMemoized(ItemPositionsListener.create);
    // Replies load newest first; older pages fold in on scroll-up.
    final hasOlderReplies = windowState?.hasOlder ?? false;
    final isLoadingOlderReplies = useState(false);
    // The list is bottom-anchored, so an open starts at the newest reply. The
    // head sits at the top end and paints only once the relay page says what
    // precedes the newest replies; until then a loading row holds its place.
    final showsHead = relayRepliesAvailable || relayReplyState.hasError;
    // While the user reads above the newest reply, arrivals wait below this
    // cutoff: inserting them at the bottom of a reversed list would push the
    // reply being read away. Reaching the bottom (or Latest) shows them.
    final newestShownCutoff = useState<TimelineMessage?>(null);
    final localPubkey = currentPubkey?.toLowerCase();
    final cutoff = newestShownCutoff.value;
    final hasPendingLocalReply =
        cutoff != null &&
        localPubkey != null &&
        allReplies.any(
          (reply) =>
              _isNewerThan(reply, cutoff) &&
              reply.pubkey.toLowerCase() == localPubkey,
        );
    final replies = cutoff == null || hasPendingLocalReply
        ? allReplies
        : [
            for (final reply in allReplies)
              if (!_isNewerThan(reply, cutoff)) reply,
          ];
    final hasPendingReplies = replies.length < allReplies.length;
    final newestReplyId = replies.lastOrNull?.id;
    final deepLinkPagesWalked = useRef(0);
    final listViewport = useMemoized(LaidOutViewport.new);
    useEffect(() => listViewport.dispose, [listViewport]);
    final stickyDateHeaderState = useValueNotifier(
      StickyDateHeaderState.hidden,
    );
    final stickyDayTimestamp = useValueNotifier<int?>(null);
    final didJumpToInitialMessage = useRef(false);
    final initialHighlightTargetIndex = useState<int?>(null);
    final isAtLatest = useState(true);
    final isNavigatingToLatest = useState(false);
    // Set by a user drag or an index jump. Layout alone (a viewport height
    // reported a frame late, a growing composer) never detaches the list.
    final userDetached = useRef(false);
    // A drag away from the newest reply is a deliberate read; focusing the
    // composer then leaves the reader where they are.
    final detachedByDrag = useRef(false);
    final activeScrollPosition = useRef<ScrollPosition?>(null);
    // An index jump or animated scroll away from the newest reply still
    // reports the old, at-latest positions until it is laid out; those must
    // not reattach the list.
    final programmaticScrollInFlight = useRef(false);
    void detachForProgrammaticScroll() {
      userDetached.value = true;
      programmaticScrollInFlight.value = true;
    }

    void settleProgrammaticScroll() {
      // The frame that lays out the new position runs first.
      WidgetsBinding.instance.addPostFrameCallback((_) {
        programmaticScrollInFlight.value = false;
      });
      WidgetsBinding.instance.scheduleFrame();
    }

    final deepLinkJumpScheduled = useRef(false);
    final olderLoadFailedAt = useRef<DateTime?>(null);
    final viewportHeight = useListenable(listViewport.height).value;
    final settledImeLift = usesFixedAndroidImeViewport
        ? (settledImeBottomInset.value -
                  MediaQuery.viewPaddingOf(context).bottom)
              .clamp(0.0, double.infinity)
              .toDouble()
        : 0.0;
    // A reversed list measures from its bottom padding, so changing it moves
    // every reply on screen. Follow the dock and keyboard only at the newest
    // reply; while reading above it, keep the inset it had when it left.
    final liveBottomInset = composerDockHeight.value + settledImeLift;
    final detachedBottomInset = useRef(liveBottomInset);
    if (isAtLatest.value) detachedBottomInset.value = liveBottomInset;
    final timelineBottomInset = isAtLatest.value
        ? liveBottomInset
        : detachedBottomInset.value;
    final navigationBottomInset = composerDockHeight.value + settledImeLift;

    // Reversed list: item 0 is the newest reply at the bottom; reply `i`
    // (chronological) lives at `replies.length - 1 - i`; the head (or its
    // loading row) is the last, top-most item.
    final headIndex = replies.length;
    // Indexes over every reply: a target held back below the cutoff is
    // revealed (the cutoff cleared) so the jump lands on it.
    int? indexForMessage(String messageId) {
      if (messageId == liveHead.id) {
        return showsHead ? headIndex : null;
      }
      final chronologicalIndex = allReplies.indexWhere(
        (reply) => reply.id == messageId,
      );
      if (chronologicalIndex < 0) return null;
      if (chronologicalIndex < replies.length) {
        return replies.length - 1 - chronologicalIndex;
      }
      newestShownCutoff.value = null;
      return allReplies.length - 1 - chronologicalIndex;
    }

    final stickyDateIndex = _ThreadStickyDateIndex(
      head: liveHead,
      replies: replies,
    );

    void updateStickyDateHeader(Iterable<ItemPosition> positions) {
      final update = stickyDateIndex.resolve(
        // The index works top-down from the head; mirror the reversed list.
        positions: [
          for (final position in positions)
            ItemPosition(
              index: headIndex - position.index,
              itemLeadingEdge: 1 - position.itemTrailingEdge,
              itemTrailingEdge: 1 - position.itemLeadingEdge,
            ),
        ],
        viewportHeight: viewportHeight,
        stickyTop: frostedAppBarHeight(context) + Grid.twelve,
        stickyHeaderHeight: StickyDateHeader.heightOf(context),
      );
      stickyDateHeaderState.value = update.state;
      stickyDayTimestamp.value = update.activeDayTimestamp;
    }

    // Item 0's leading edge is the visible bottom boundary above the composer.
    double latestAlignment() => viewportHeight > 0
        ? ((Grid.xs + timelineBottomInset) / viewportHeight)
              .clamp(0.0, 1.0)
              .toDouble()
        : 0.0;

    bool latestIsAtBoundary(Iterable<ItemPosition> positions) {
      final boundary = latestAlignment();
      return positions.any(
        (position) =>
            position.index == 0 && position.itemLeadingEdge >= boundary - 0.01,
      );
    }

    void requestOlderReplies() {
      if (isLoadingOlderReplies.value) return;
      isLoadingOlderReplies.value = true;
      unawaited(
        ref
            .read(threadWindowProvider(repliesArgs).notifier)
            .loadOlder()
            .catchError((Object _) => false)
            .then((loaded) {
              olderLoadFailedAt.value = loaded ? null : DateTime.now();
            })
            .whenComplete(() {
              if (context.mounted) isLoadingOlderReplies.value = false;
            }),
      );
    }

    void maybeLoadOlderReplies(Iterable<ItemPosition> positions) {
      // Only a laid-out relay page can be near its top; the loading row that
      // stands in for the head while it is pending is not.
      if (!showsHead || !hasOlderReplies || isLoadingOlderReplies.value) {
        return;
      }
      // A failed page is retried on scroll after a pause, not every frame.
      final failedAt = olderLoadFailedAt.value;
      if (failedAt != null &&
          DateTime.now().difference(failedAt) < const Duration(seconds: 3)) {
        return;
      }
      // Older pages land above the top-most item, away from the bottom
      // anchor, so the replies on screen stay where they are.
      final nearTop = positions.any(
        (position) =>
            position.index >= headIndex - 1 && position.itemTrailingEdge > 0,
      );
      if (nearTop) requestOlderReplies();
    }

    useEffect(
      () {
        void onPositionsChanged() {
          final positions = itemPositionsListener.itemPositions.value;
          if (positions.isEmpty) return;
          updateStickyDateHeader(positions);
          maybeLoadOlderReplies(positions);
          if (isNavigatingToLatest.value) return;
          final atLatest = latestIsAtBoundary(positions);
          if (atLatest && programmaticScrollInFlight.value) return;
          if (atLatest) {
            userDetached.value = false;
            detachedByDrag.value = false;
            if (!isAtLatest.value) isAtLatest.value = true;
            newestShownCutoff.value = null;
          } else if (userDetached.value) {
            if (isAtLatest.value) isAtLatest.value = false;
            newestShownCutoff.value ??= replies.lastOrNull;
          }
        }

        itemPositionsListener.itemPositions.addListener(onPositionsChanged);
        return () => itemPositionsListener.itemPositions.removeListener(
          onPositionsChanged,
        );
      },
      [
        itemPositionsListener,
        replies.length,
        liveHead.createdAt,
        viewportHeight,
        hasOlderReplies,
        timelineBottomInset,
        newestReplyId,
      ],
    );

    useEffect(() {
      stickyDateHeaderState.value = StickyDateHeaderState.hidden;
      stickyDayTimestamp.value = null;
      return null;
    }, [threadHead.id]);

    void scrollToThreadLatest({bool animate = true}) {
      if (!itemScrollController.isAttached) return;
      // An explicit Latest supersedes a still-pending Inbox/Activity deep
      // link, so a late thread query cannot jump back to the linked reply.
      didJumpToInitialMessage.value = true;
      initialHighlightTargetIndex.value = null;
      initialTargetReadyForHighlight.value = false;
      newestShownCutoff.value = null;
      isNavigatingToLatest.value = true;
      final disableAnimations =
          !animate || MediaQuery.disableAnimationsOf(context);
      WidgetsBinding.instance.addPostFrameCallback((_) async {
        try {
          if (!context.mounted || !itemScrollController.isAttached) return;
          // Glide the one active position toward the bottom, like the
          // channel's visible glide but without the package's cross-faded
          // second list, then settle exactly on item 0. Lazy extents can
          // still grow during the glide; the final jump absorbs that.
          final position = activeScrollPosition.value;
          if (!disableAnimations &&
              position != null &&
              position.hasContentDimensions) {
            await position.animateTo(
              position.minScrollExtent,
              duration: jumpToLatestScrollDuration,
              curve: jumpToLatestScrollCurve,
            );
          }
          if (context.mounted && itemScrollController.isAttached) {
            itemScrollController.jumpTo(index: 0);
          }
        } finally {
          if (context.mounted) {
            isNavigatingToLatest.value = false;
            isAtLatest.value = true;
          }
        }
      });
    }

    // A reply the current user just sent is shown even while they read
    // further up: sending returns them to the newest reply.
    useEffect(() {
      if (hasPendingLocalReply) scrollToThreadLatest(animate: false);
      return null;
    }, [hasPendingLocalReply]);

    // Keep the newest reply on the bottom boundary when the composer, the
    // keyboard, or a new reply changes the layout while the user is there.
    // Item 0 normally stays put on its own; this covers a list last moved by
    // an index jump.
    void realignLatestAfterLayoutChange() {
      if (!isAtLatest.value || isNavigatingToLatest.value) return;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!context.mounted ||
            !itemScrollController.isAttached ||
            !isAtLatest.value ||
            isNavigatingToLatest.value ||
            latestIsAtBoundary(itemPositionsListener.itemPositions.value)) {
          return;
        }
        // Index 0 at alignment 0 already sits on the list's bottom padding.
        itemScrollController.jumpTo(index: 0);
      });
    }

    useEffect(() {
      realignLatestAfterLayoutChange();
      return null;
    }, [newestReplyId, timelineBottomInset, viewportHeight]);

    final quoteJumpHighlightTimer = useRef<Timer?>(null);
    useEffect(
      () =>
          () => quoteJumpHighlightTimer.value?.cancel(),
      const [],
    );

    // A quote of a row this thread already shows scrolls in place and flashes
    // the deep-link landing highlight instead of pushing a duplicate thread.
    bool jumpToQuotedMessage(String messageId) {
      final targetIndex = indexForMessage(messageId);
      if (targetIndex == null || !itemScrollController.isAttached) {
        return false;
      }
      // Like Latest, this explicit navigation supersedes a pending deep link
      // and its landing highlight.
      didJumpToInitialMessage.value = true;
      initialHighlightTargetIndex.value = null;
      initialTargetReadyForHighlight.value = false;
      detachForProgrammaticScroll();
      unawaited(
        itemScrollController
            .scrollTo(
              index: targetIndex,
              alignment: 0.35,
              duration: const Duration(milliseconds: 220),
              curve: Curves.easeOutCubic,
            )
            .whenComplete(settleProgrammaticScroll),
      );
      highlightedMessageId.value = messageId;
      quoteJumpHighlightTimer.value?.cancel();
      quoteJumpHighlightTimer.value = Timer(
        _landingHighlightDuration +
            (reducedLandingHighlightMotion
                ? Duration.zero
                : _landingHighlightTransitionDuration),
        () {
          if (context.mounted) highlightedMessageId.value = null;
        },
      );
      return true;
    }

    useEffect(
      () {
        final messageId = initialMessageId;
        // Wait for either the authoritative thread query or a terminal query
        // error before consuming the one-shot jump. During loading, the fallback
        // main-timeline list can contain only the linked reply; after an error,
        // that hydrated snapshot is the best available target list.
        if (messageId == null || !canUseMessagesForInitialTarget) return null;
        final targetIndex = indexForMessage(messageId);
        if (targetIndex == null) {
          // Replies load newest first: walk back a few pages for a linked
          // reply that is older than what is loaded, as desktop does.
          if (relayRepliesAvailable &&
              hasOlderReplies &&
              deepLinkPagesWalked.value < _deepLinkMaxOlderPages &&
              !isLoadingOlderReplies.value) {
            deepLinkPagesWalked.value++;
            requestOlderReplies();
          }
          return null;
        }
        if (didJumpToInitialMessage.value || deepLinkJumpScheduled.value) {
          return null;
        }
        deepLinkJumpScheduled.value = true;
        WidgetsBinding.instance.addPostFrameCallback((_) {
          deepLinkJumpScheduled.value = false;
          if (!context.mounted ||
              !itemScrollController.isAttached ||
              didJumpToInitialMessage.value) {
            return;
          }
          didJumpToInitialMessage.value = true;
          detachForProgrammaticScroll();
          itemScrollController.jumpTo(index: targetIndex, alignment: 0.35);
          settleProgrammaticScroll();
          initialHighlightTargetIndex.value = targetIndex;
        });
        return null;
      },
      [
        initialMessageId,
        canUseMessagesForInitialTarget,
        fetchedReplies,
        replies.length,
        hasOlderReplies,
        isLoadingOlderReplies.value,
      ],
    );

    useEffect(
      () {
        final targetIndex = initialHighlightTargetIndex.value;
        if (targetIndex == null || initialTargetReadyForHighlight.value) {
          return null;
        }
        var completionScheduled = false;
        void markReadyAfterTargetLayout() {
          if (completionScheduled ||
              !itemPositionsListener.itemPositions.value.any(
                (position) => position.index == targetIndex,
              )) {
            return;
          }
          completionScheduled = true;
          WidgetsBinding.instance.addPostFrameCallback((_) {
            if (context.mounted) initialTargetReadyForHighlight.value = true;
          });
        }

        itemPositionsListener.itemPositions.addListener(
          markReadyAfterTargetLayout,
        );
        markReadyAfterTargetLayout();
        return () => itemPositionsListener.itemPositions.removeListener(
          markReadyAfterTargetLayout,
        );
      },
      [initialHighlightTargetIndex.value, initialTargetReadyForHighlight.value],
    );

    final readState = ref.watch(readStateProvider);
    final visibleReplyReadKey = replies
        .map((reply) => '${reply.id}:${reply.createdAt}')
        .join(',');

    useEffect(() {
      if (!readState.isReady || replies.isEmpty) return null;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        for (final reply in replies) {
          ref
              .read(readStateProvider.notifier)
              .markContextRead(msgContextKey(reply.id), reply.createdAt);
        }
      });
      return null;
    }, [threadHead.id, readState.isReady, visibleReplyReadKey]);

    // Thread-scoped typing indicators (exclude self).
    final allTyping = ref.watch(channelTypingProvider(channelId));
    final threadTyping = allTyping
        .where((e) => e.threadHeadId == threadHead.id)
        .where(
          (e) =>
              currentPubkey == null ||
              e.pubkey.toLowerCase() != currentPubkey?.toLowerCase(),
        )
        .toList();

    // The root of the entire thread chain. If the current thread head is
    // itself a root message its rootId is null, so fall back to its own id.
    final effectiveRootId = threadHead.rootId ?? threadHead.id;
    final goalsEnabled = ref.watch(goalsEnabledProvider);

    void updateComposerDockHeight(double height) {
      listViewport.reportAfterLayout();
      if ((height - composerDockHeight.value).abs() < 0.5) return;
      composerDockHeight.value = height;
    }

    useEffect(() {
      final observer = ImeMetricsSettleObserver(
        onMetricsSettled: () {
          listViewport.reportAfterLayout();
          if (!usesFixedAndroidImeViewport) return;
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

    // Channel names for message content rendering.
    final channelsAsync = ref.watch(channelsProvider);
    final channel = channelsAsync.value
        ?.where((candidate) => candidate.id == channelId)
        .firstOrNull;
    final channelNamesMap = <String, String>{};
    channelsAsync.whenData((channels) {
      for (final ch in channels) {
        channelNamesMap[ch.name.toLowerCase()] = ch.id;
      }
    });
    final usesNativeIosGlassBackButton =
        Navigator.canPop(context) &&
        Theme.of(context).platform == TargetPlatform.iOS;

    return FrostedScaffold(
      resizeToAvoidBottomInset: !usesFixedAndroidImeViewport,
      appBar: FrostedAppBar(
        leading: usesNativeIosGlassBackButton
            ? IosGlassNavigationButton(
                key: const ValueKey('thread-ios-glass-back'),
                icon: IosGlassNavigationIcon.back,
                semanticLabel: 'Back',
                onPressed: () => Navigator.of(context).maybePop(),
                width: iosGlassChannelHeaderLeadingWidth,
                buttonCenterX: iosGlassChannelHeaderButtonCenterX,
                nativeViewSuppressed: messageActionBackdropActive,
              )
            : null,
        iconColor: context.colors.primary,
        title: Padding(
          padding: EdgeInsets.only(
            left: usesNativeIosGlassBackButton
                ? iosGlassChannelHeaderTitleSpacing
                : 0,
          ),
          child: Text(
            threadName == null || threadName.isEmpty ? 'Thread' : threadName,
            key: const ValueKey('thread-app-bar-title'),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
        ),
        actions: [
          if (goalsEnabled)
            ThreadGoalsButton(
              channelId: channelId,
              threadRootId: effectiveRootId,
            ),
          if (isMember && !isArchived && !liveDeletionHidesHead)
            IconButton(
              tooltip: 'Edit thread name',
              icon: const Icon(Icons.edit_outlined),
              onPressed: () => showThreadNameEditor(context, nameKey),
            ),
        ],
        titleStyle: channelTitleTextStyle,
      ),
      body: Stack(
        fit: StackFit.expand,
        children: [
          Column(
            children: [
              Expanded(
                child: QuoteJumpScope(
                  channelId: channelId,
                  jumpToMessage: jumpToQuotedMessage,
                  child: _ThreadMessageList(
                    viewport: listViewport,
                    onUserScrollStart: () {
                      userDetached.value = true;
                      detachedByDrag.value = true;
                    },
                    activeScrollPosition: activeScrollPosition,
                    itemScrollController: itemScrollController,
                    itemPositionsListener: itemPositionsListener,
                    bottomInset: timelineBottomInset,
                    replies: replies,
                    relayReplyState: relayReplyState,
                    hasOlderReplies: hasOlderReplies,
                    isLoadingOlderReplies: isLoadingOlderReplies.value,
                    onRetryReplies: () =>
                        ref.invalidate(threadRepliesProvider(repliesArgs)),
                    localSendAnimations: localSendAnimations,
                    showsHead: showsHead,
                    headIsDeleted: liveDeletionHidesHead,
                    head: liveHead,
                    stickyDayTimestamp: stickyDayTimestamp,
                    channelNames: channelNamesMap,
                    channelId: channelId,
                    currentPubkey: currentPubkey,
                    highlightedMessageId: highlightedMessageId.value,
                    allMessages: allMsgs,
                    isMember: isMember,
                    isArchived: isArchived,
                    composerFocusNode: composerFocusNode,
                    restoreComposerFocus: () =>
                        restoreComposerFocus.value?.call(),
                    childrenByParent: childrenByParent,
                    quoteScope: isMember && !isArchived ? quoteScope : null,
                  ),
                ),
              ),
            ],
          ),
          Positioned(
            left: 0,
            right: 0,
            top: frostedAppBarHeight(context) + Grid.twelve,
            child: StickyDateHeader(
              key: const ValueKey('thread-sticky-date-header'),
              state: stickyDateHeaderState,
            ),
          ),
          // Without a composer the typing row still docks over the list's
          // bottom, so it pads the newest reply like the composer does
          // instead of resizing the list under a reader further up.
          if (!isMember || isArchived)
            Align(
              alignment: Alignment.bottomCenter,
              child: ComposerDockSizeReporter(
                key: const ValueKey('thread-typing-dock'),
                onHeightChanged: updateComposerDockHeight,
                child: _ThreadTypingIndicator(
                  channelId: channelId,
                  entries: threadTyping,
                  animated: false,
                ),
              ),
            ),
          if (isMember && !isArchived)
            AndroidImeLift(
              child: Align(
                alignment: Alignment.bottomCenter,
                child: ComposerDockSizeReporter(
                  key: const ValueKey('thread-composer-dock'),
                  onHeightChanged: updateComposerDockHeight,
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      _ThreadTypingIndicator(
                        channelId: channelId,
                        entries: threadTyping,
                      ),
                      ComposerQuoteChip(scope: quoteScope),
                      ComposeBar(
                        channelId: channelId,
                        focusNode: composerFocusNode,
                        onFocusRestorerChanged: (restoreFocus) =>
                            restoreComposerFocus.value = restoreFocus,
                        hintText: 'Reply in thread\u2026',
                        threadHeadId: threadHead.id,
                        rootId: effectiveRootId,
                        threadRootTags: threadHead.tags,
                        // Replying is about the newest replies: focusing the
                        // composer while reading further up returns there.
                        onFocusRequested: () {
                          if (detachedByDrag.value) return;
                          if (!isAtLatest.value || hasPendingReplies) {
                            scrollToThreadLatest(animate: false);
                          }
                        },
                        onSend:
                            (
                              content,
                              mentionPubkeys, {
                              mediaTags = const <List<String>>[],
                            }) async {
                              // Thread reply tags place the message; the
                              // quote only adds a `q` reference.
                              await sendMessage.call(
                                channelId: channelId,
                                content: content,
                                mentionPubkeys: mentionPubkeys,
                                channel: channel,
                                parentEventId: threadHead.id,
                                rootEventId: effectiveRootId,
                                mediaTags: mediaTags,
                                quote: pendingQuote,
                              );
                              if (pendingQuote != null && context.mounted) {
                                ref
                                    .read(
                                      composerQuoteProvider(
                                        quoteScope,
                                      ).notifier,
                                    )
                                    .clearIf(pendingQuote);
                              }
                            },
                      ),
                    ],
                  ),
                ),
              ),
            ),
          Positioned(
            left: 0,
            right: 0,
            bottom: navigationBottomInset + Grid.xs,
            child: Center(
              child: JumpToLatestSwitcher(
                id: 'thread',
                visible:
                    hasFetchedReplies &&
                    !isNavigatingToLatest.value &&
                    (!isAtLatest.value || hasPendingReplies),
                onPressed: scrollToThreadLatest,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
