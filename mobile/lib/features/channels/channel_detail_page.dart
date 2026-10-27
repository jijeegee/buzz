import 'dart:async';
import '../../shared/read_aloud/read_aloud_message.dart';
import 'dart:math' show cos, max, min, pi;
import 'dart:ui' show ImageFilter, lerpDouble;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart' show ScrollDirection;
import 'package:flutter/services.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:scrollable_positioned_list/scrollable_positioned_list.dart';

import '../../shared/devices/agent_badge_widgets.dart';

import '../../shared/animated_avatar.dart';
import '../../shared/emoji/emoji_burst.dart';
import '../../shared/huddle/huddle.dart';
import '../../shared/identity_names/identity_names.dart';
import '../../shared/identity_names/identity_names_provider.dart';
import '../../shared/mentions/agent_identity_provider.dart';
import '../../shared/relay/relay.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../agents/owner_view/owner_view.dart';
import '../agents/owner_view/owner_view_seen.dart';
import '../agents/owner_view/owner_view_widgets.dart';
import '../chats/chats_page.dart' show AgentConversationsList;
import '../goals/goal_provider.dart';
import '../goals/goals_page.dart';
import '../groups/group_preview.dart' show GroupJoinBar;
import '../../shared/widgets/avatar_image.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/widgets/bouncing_dots_indicator.dart';
import '../../shared/widgets/concentric_sheet_surface.dart';
import '../../shared/widgets/frosted_app_bar.dart';
import '../../shared/widgets/frosted_scaffold.dart';
import '../../shared/widgets/flapping_bee.dart';
import '../../shared/widgets/keyboard_dismiss_on_drag.dart';
import '../../shared/widgets/kuzz_view_switch.dart';
import '../../shared/widgets/load_error_view.dart';
import '../../shared/widgets/ios_glass_navigation_button.dart';
import '../../shared/widgets/masked_avatar_badge.dart';
import '../../shared/widgets/message_author_meta.dart';
import '../../shared/widgets/modal_presentation.dart';
import '../../shared/widgets/skeleton.dart';
import '../profile/presence_cache_provider.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/profile/user_profile.dart';
import '../forum/forum_posts_view.dart';
import 'android_ime_lift.dart';
import 'channel.dart';
import 'group_avatar.dart';
import 'channel_actions_sheet.dart';
import 'channel_identity_names_provider.dart';
import 'channel_link_navigation.dart';
import 'agent_activity/working_bots_provider.dart';
import 'channel_management_provider.dart';
import 'channel_sections/channel_sections_provider.dart';
import 'channel_messages_provider.dart';
import 'channel_typing_provider.dart';
import 'channel_typing_indicator.dart';
import 'channels_provider.dart';
import 'unread_badge/observed_unread_event.dart';
import 'compose_bar.dart';
import 'composer_quote_chip.dart';
import 'composer_quote_provider.dart';
import 'composer_dock_size_reporter.dart';
import 'chat_bubble_row.dart';
import 'date_formatters.dart';
import 'day_divider.dart';
import 'dm_channel_labels.dart';
import 'ephemeral_channel_display.dart';
import 'emoji_picker.dart';
import 'ime_metrics_settle_observer.dart';
import 'jump_to_latest_button.dart';
import 'jump_to_latest_switcher.dart';
import 'failed_local_messages_provider.dart';
import 'local_message_send_animation_provider.dart';
import 'local_send_status_mark.dart';
import 'local_message_send_transition.dart';
import 'mobile_huddle_controller.dart';
import 'members_sheet.dart';
import 'message_actions.dart';
import 'message_action_backdrop_state.dart';
import 'message_long_press_region.dart';
import 'message_content.dart';
import 'message_quote.dart';
import 'message_quote_header.dart';
import 'new_messages_divider.dart';
import '../../shared/read_state/deferred_read_state_update.dart';
import '../../shared/read_state/read_state_format.dart';
import '../../shared/read_state/read_state_provider.dart';
import '../../shared/read_state/read_state_time.dart';
import 'reaction_row.dart';
import 'recent_emoji_provider.dart';
import 'room_position_store.dart';
import 'send_message_provider.dart';
import '../people/blocked_dm_bar.dart';
import '../people/blocked_message_gate.dart';
import '../people/blocked_users_provider.dart';
import '../profile/user_profile_sheet.dart';
import 'small_avatar.dart';
import 'sticky_date_header.dart';
import 'thread_depth.dart';
import 'thread_detail_page.dart';
import 'thread_replies_provider.dart';
import 'thread_name_provider.dart';
import 'timeline_message.dart';

part 'channel_detail_page/message_list.dart';
part 'channel_detail_page/system_rows.dart';
part 'channel_detail_page/huddle_sheet.dart';
part 'channel_detail_page/huddle_call_avatar.dart';
part 'channel_detail_page/huddle_participant_cluster.dart';
part 'channel_detail_page/huddle_call_participants.dart';
part 'channel_detail_page/huddle_participant_overlay.dart';
part 'channel_detail_page/huddle_call_controls.dart';
part 'channel_detail_page/huddle_drawer.dart';
part 'channel_detail_page/huddle_reactions.dart';
part 'channel_detail_page/message_bubble.dart';
part 'channel_detail_page/banners.dart';
part 'channel_detail_page/app_bar.dart';
part 'channel_detail_page/direct_thread.dart';

/// Fetch deep-link targets that may be outside the loaded channel window.
Future<void> _loadDeepLinkEvents(
  WidgetRef ref,
  String channelId,
  Set<String> eventIds,
) async {
  try {
    await ref
        .read(channelMessagesProvider(channelId).notifier)
        .loadEventsById(eventIds);
  } catch (error) {
    debugPrint('deep-link: failed to load target messages: $error');
  }
}

/// Fetch channel members and preload their profiles into the user cache.
/// One-to-one DMs additionally refresh participant profiles for identity gates.
/// Returns whether identity resolution completed successfully.
Future<bool> _preloadMembers(
  WidgetRef ref,
  String channelId,
  List<String> participantPubkeys, {
  required bool refreshDmParticipants,
}) async {
  // Capture references before async gap to avoid using disposed ref.
  final notifier = ref.read(userCacheProvider.notifier);
  try {
    final members = await ref.read(channelMembersProvider(channelId).future);
    await notifier.preload(members.map((member) => member.pubkey).toList());
    if (refreshDmParticipants) {
      return notifier.refresh(participantPubkeys);
    }
    return true;
  } catch (_) {
    // Identity remains unresolved, so agent-only actions stay hidden.
    return false;
  }
}

Future<void Function()> _subscribeToDmIdentityUpdates(
  WidgetRef ref,
  List<String> participantPubkeys, {
  required ValueChanged<bool> onReadyChanged,
  required ValueChanged<Set<String>> onAgentPubkeysChanged,
  required VoidCallback onFailure,
}) async {
  final session = ref.read(relaySessionProvider.notifier);
  var subscriptionStatus = RelaySubscriptionStatus.retrying;
  var directLookupComplete = false;
  final agentPubkeys = <String>{};

  void publishAgentPubkeys() {
    onAgentPubkeysChanged(Set.unmodifiable(agentPubkeys));
  }

  void handleEvent(NostrEvent event) {
    if (event.kind == 0) {
      try {
        ref.read(userCacheProvider.notifier).cacheProfileEvent(event);
      } catch (error) {
        debugPrint('[DmIdentity] invalid live profile: $error');
        onFailure();
      }
    } else if (event.kind == 10100) {
      agentPubkeys.add(event.pubkey.toLowerCase());
      publishAgentPubkeys();
      ref.invalidate(agentDirectoryProvider);
      ref.invalidate(agentOwnersProvider);
    }
  }

  final unsubscribe = await session.subscribeWithStatus(
    NostrFilter(
      kinds: const [0, 10100],
      authors: participantPubkeys,
      limit: 100,
    ).copyWithSince(DateTime.now().millisecondsSinceEpoch ~/ 1000 - 5),
    handleEvent,
    onClosed: (_) => onFailure(),
    onStatusChanged: (status) {
      subscriptionStatus = status;
      if (status == RelaySubscriptionStatus.retrying) {
        onReadyChanged(false);
      } else if (directLookupComplete) {
        onReadyChanged(true);
      }
    },
  );

  try {
    final profiles = await session.fetchHistory(
      NostrFilter(
        kinds: const [10100],
        authors: participantPubkeys,
        limit: participantPubkeys.length,
      ),
    );
    for (final profile in profiles) {
      if (profile.kind == 10100) {
        agentPubkeys.add(profile.pubkey.toLowerCase());
      }
    }
    publishAgentPubkeys();
    directLookupComplete = true;
    onReadyChanged(subscriptionStatus == RelaySubscriptionStatus.ready);
    return unsubscribe;
  } catch (_) {
    unsubscribe();
    rethrow;
  }
}

int? _channelReadTimestamp({
  required Channel channel,
  required AsyncValue<List<NostrEvent>> messagesState,
  List<NostrEvent>? heldNewerEvents,
}) {
  if (channel.isForum) {
    return dateTimeToUnixSeconds(channel.lastMessageAt);
  }

  final events = messagesState.value;
  // A window opened at a message stops short of the present; opening the
  // channel still reads everything up to its newest known row.
  if (heldNewerEvents != null) {
    var latest = dateTimeToUnixSeconds(channel.lastMessageAt) ?? 0;
    for (final event in [...?events, ...heldNewerEvents]) {
      if (event.threadReference.parentId != null) continue;
      if (event.createdAt > latest) latest = event.createdAt;
    }
    return latest > 0 ? latest : null;
  }
  if (events != null && events.isNotEmpty) {
    var latest = 0;
    for (final event in events) {
      if (event.threadReference.parentId != null) continue;
      if (event.createdAt > latest) {
        latest = event.createdAt;
      }
    }
    if (latest > 0) {
      return latest;
    }
  }

  return dateTimeToUnixSeconds(channel.lastMessageAt);
}

bool _isOneToOneAgentDm(Channel channel, Set<String> agentPubkeys) {
  final participants = channel.participantPubkeys
      .map((pubkey) => pubkey.trim().toLowerCase())
      .where((pubkey) => pubkey.isNotEmpty)
      .toSet();
  return channel.isDm &&
      participants.length == 2 &&
      participants.any(agentPubkeys.contains);
}

/// Controls how a hydrated initial thread is added to the navigation stack.
enum InitialThreadRouteBehavior {
  /// Keep the channel route beneath the thread.
  push,

  /// Replace the temporary channel route so Back returns to its origin.
  replaceCurrentRoute,
}

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

/// Channel pages on screen or under other routes, oldest first.
final _openChannelPages = <BuildContext>{};

/// The route of the newest open page of [channelId], if any (for returning
/// to a channel instead of stacking another copy of it).
ModalRoute<Object?>? openChannelRoute(String channelId) {
  for (final page in _openChannelPages.toList().reversed) {
    if (!page.mounted) continue;
    if ((page.widget as ChannelDetailPage).channel.id != channelId) continue;
    final route = ModalRoute.of(page);
    if (route != null && route.isActive) return route;
  }
  return null;
}

class ChannelDetailPage extends HookConsumerWidget {
  final Channel channel;
  final String? initialMessageId;
  final String? initialThreadRootId;

  /// How the automatically opened initial thread affects the route stack.
  final InitialThreadRouteBehavior initialThreadRouteBehavior;

  /// Briefly highlight [initialMessageId] on the main timeline after the
  /// jump (goal message links).
  final bool highlightInitialMessage;

  /// Set when the owner of an agent reads one of its chats without being in
  /// it: read-only, the agent's messages on the right, nothing marked read.
  final OwnerView? ownerView;

  const ChannelDetailPage({
    super.key,
    required this.channel,
    this.initialMessageId,
    this.initialThreadRootId,
    this.initialThreadRouteBehavior = InitialThreadRouteBehavior.push,
    this.highlightInitialMessage = false,
    this.ownerView,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    useEffect(() {
      _openChannelPages.add(context);
      return () => _openChannelPages.remove(context);
    }, const []);
    final composerDockHeight = useState(0.0);
    final composerFocusNode = useFocusNode();
    final restoreComposerFocus = useRef<VoidCallback?>(null);
    final sendMessage = ref.read(sendMessageProvider);
    final quoteScope = ComposerQuoteScope(channelId: channel.id);
    // Captured per build so a send carries the quote shown when it started.
    final pendingQuote = ref.watch(composerQuoteProvider(quoteScope));
    final detailsAsync = ref.watch(channelDetailsProvider(channel.id));
    final listedChannel = ref.watch(
      channelsProvider.select(selectChannel(channel.id)),
    );
    final messagesState = ref.watch(channelMessagesProvider(channel.id));
    final opensThreadDirectly =
        initialThreadRootId != null &&
        !channel.isForum &&
        initialThreadRouteBehavior ==
            InitialThreadRouteBehavior.replaceCurrentRoute;
    final didOpenDirectThread = useRef(false);
    final deepLinkLoads = useState(0);
    final huddleLifecycle =
        ref.watch(huddleLifecycleProvider(channel.id)).value ?? const [];
    final sessionStatus = ref.watch(relaySessionProvider).status;
    final readState = ref.watch(readStateProvider);
    final channelsNotifier = ref.read(channelsProvider.notifier);
    final initialOrdinaryUnreadMessageIdsRef = useRef<Set<String>>(const {});
    final initialOldestOrdinaryUnreadMessageIdRef = useRef<String?>(null);
    final initialOldestOrdinaryUnreadCreatedAtRef = useRef<int?>(null);
    final initialForcedUnreadMessageIdsRef = useRef<Set<String>>(const {});
    final didCaptureInitialReadAt = useRef(false);
    if (readState.isReady && !didCaptureInitialReadAt.value) {
      final channelReadAt = readState.effectiveTimestamp(channel.id);
      final ordinaryUnreadEvents = [
        for (final event
            in channelsNotifier
                    .observedUnreadEventsByChannel[channel.id]
                    ?.values ??
                const <ObservedUnreadEvent>[])
          if (event.rootId == null &&
              event.createdAt >
                  (observedUnreadEventReadAt(
                        event,
                        channelReadAt,
                        (rootId) => readState.effectiveTimestamp(
                          threadContextKey(rootId),
                        ),
                        (messageId) => readState.effectiveTimestamp(
                          msgContextKey(messageId),
                        ),
                      ) ??
                      0))
            event,
      ]..sort((a, b) => a.createdAt.compareTo(b.createdAt));
      initialOrdinaryUnreadMessageIdsRef.value = {
        for (final event in ordinaryUnreadEvents) event.id,
      };
      initialOldestOrdinaryUnreadMessageIdRef.value =
          ordinaryUnreadEvents.firstOrNull?.id;
      initialOldestOrdinaryUnreadCreatedAtRef.value =
          ordinaryUnreadEvents.firstOrNull?.createdAt;
      initialForcedUnreadMessageIdsRef.value = {
        for (final entry in readState.forcedUnreadContexts.entries)
          if (entry.value == channel.id && entry.key.startsWith('msg:'))
            entry.key.substring('msg:'.length),
      };
      didCaptureInitialReadAt.value = true;
    }
    final initialOrdinaryUnreadMessageIds =
        initialOrdinaryUnreadMessageIdsRef.value;
    final initialOldestOrdinaryUnreadMessageId =
        initialOldestOrdinaryUnreadMessageIdRef.value;
    final initialForcedUnreadMessageIds =
        initialForcedUnreadMessageIdsRef.value;
    final currentPubkey = ref.watch(currentPubkeyProvider);
    // Only show channel-level typing (exclude thread-scoped entries and self).
    final typingEntries = ref
        .watch(channelTypingProvider(channel.id))
        .where((e) => e.threadHeadId == null)
        .where(
          (e) =>
              currentPubkey == null ||
              e.pubkey.toLowerCase() != currentPubkey.toLowerCase(),
        )
        .toList();
    final baseChannel = listedChannel ?? channel;
    final shownOwnerView = this.ownerView;
    final mergedChannel =
        detailsAsync.whenData(baseChannel.mergeDetails).value ?? baseChannel;
    // The owner view is never a member: no composer, join or huddle.
    final resolvedChannel = shownOwnerView == null
        ? mergedChannel
        : mergedChannel.copyWith(isMember: false);
    final participantCount = resolvedChannel.participantPubkeys
        .map((pubkey) => pubkey.trim().toLowerCase())
        .where((pubkey) => pubkey.isNotEmpty)
        .toSet()
        .length;
    final isOneToOneDm = resolvedChannel.isDm && participantCount == 2;
    final memberProfilesPreload = useMemoized(
      () => _preloadMembers(
        ref,
        resolvedChannel.id,
        resolvedChannel.participantPubkeys,
        refreshDmParticipants: isOneToOneDm,
      ),
      [
        resolvedChannel.id,
        sessionStatus,
        isOneToOneDm,
        Object.hashAll(resolvedChannel.participantPubkeys),
      ],
    );
    final memberProfilesPreloadState = useFuture(memberProfilesPreload);
    // The DM with one of my agents also lists the agent's chats without me.
    final dmAgentPubkey = shownOwnerView == null
        ? oneToOneDmCounterpart(resolvedChannel, currentPubkey)
        : null;
    final isMyAgentDm =
        dmAgentPubkey != null && ref.watch(isMyAgentProvider(dmAgentPubkey));
    final showsAgentConversations =
        isMyAgentDm &&
        ref.watch(agentDmTabProvider(channel.id)) == AgentDmTab.conversations;
    final dmAgentName = isMyAgentDm
        ? ref.watch(
            identityNameSourcesProvider.select(
              (names) => resolveDmChannelDisplayLabel(
                resolvedChannel,
                currentPubkey: currentPubkey,
                names: names,
              ),
            ),
          )
        : '';
    final ownerView = shownOwnerView?.withChatLabel(
      resolvedChannel.isGroupChat
          ? watchChannelTitle(ref, resolvedChannel)
          : resolvedChannel.isDm
          ? ref.watch(
              identityNameSourcesProvider.select(
                (names) => resolveDmChannelDisplayLabel(
                  resolvedChannel,
                  currentPubkey: shownOwnerView.agentPubkey,
                  names: names,
                ),
              ),
            )
          : resolvedChannel.name,
    );
    // Chats with something new since the owner last looked: the tab's count.
    final agentDots = isMyAgentDm
        ? ref.watch(
            agentChatRowsProvider(
              dmAgentPubkey,
            ).select((rows) => ownerViewDotCount(rows.value ?? const [])),
          )
        : 0;
    // The open chat's dot stays clear while it is read here.
    useMarkOwnerViewSeen(
      ref,
      ownerView,
      (row) => !row.isThread && row.channelId == resolvedChannel.id,
    );
    // People here are told when an agent's owner can read along.
    final ownerReaders = ownerView == null
        ? watchOwnerReaders(ref, resolvedChannel)
        : const <OwnerReader>[];
    final appBarBottomHeight =
        ownerReaders.length * OwnerCanReadNotice.lineHeight +
        (isMyAgentDm ? _agentTabsHeight + Grid.xs : 0);
    final showsComposer =
        !showsAgentConversations &&
        !resolvedChannel.isForum &&
        resolvedChannel.isMember &&
        !resolvedChannel.isArchived;
    // In a one-to-one DM with someone I blocked a bar with Unblock stands in
    // for the composer, and their typing is not shown.
    final blockedCounterpart = isOneToOneDm
        ? blockedDmCounterpart(
            resolvedChannel,
            currentPubkey: currentPubkey,
            blocked: ref.watch(blockedPubkeysProvider),
          )
        : null;
    final showsComposeBar = showsComposer && blockedCounterpart == null;
    final visibleTypingEntries = blockedCounterpart == null
        ? typingEntries
        : typingEntries
              .where((e) => e.pubkey.toLowerCase() != blockedCounterpart)
              .toList();
    final profileOwnedAgentPubkeys = <String>[];
    for (final participantPubkey in resolvedChannel.participantPubkeys) {
      final normalized = participantPubkey.trim().toLowerCase();
      final isProfileOwnedAgent = ref.watch(
        userCacheProvider.select(
          (cache) => cache[normalized]?.ownerPubkey != null,
        ),
      );
      if (isProfileOwnedAgent) profileOwnedAgentPubkeys.add(normalized);
    }
    final agentDirectoryState = ref.watch(agentDirectoryProvider);
    final agentOwnersState = ref.watch(agentOwnersProvider);
    final channelMembershipUpdateState = isOneToOneDm
        ? ref.watch(channelMembershipUpdateProvider(resolvedChannel.id))
        : const ChannelMembershipUpdateState(isReady: true);
    final channelBotPubkeysState = ref.watch(
      channelBotPubkeysProvider(resolvedChannel.id),
    );
    final identitySubscriptionPubkeys = isOneToOneDm
        ? (resolvedChannel.participantPubkeys
              .map((pubkey) => pubkey.trim().toLowerCase())
              .where((pubkey) => pubkey.isNotEmpty)
              .toSet()
              .toList()
            ..sort())
        : const <String>[];
    final identitySubscriptionKey = Object.hashAll(identitySubscriptionPubkeys);
    final identitySubscriptionReady = useValueNotifier(false, [
      sessionStatus,
      resolvedChannel.id,
      identitySubscriptionKey,
    ]);
    final directlyResolvedAgentPubkeys = useValueNotifier(<String>{}, [
      sessionStatus,
      resolvedChannel.id,
      identitySubscriptionKey,
    ]);
    final isIdentitySubscriptionReady = useValueListenable(
      identitySubscriptionReady,
    );
    final directAgentPubkeys = useValueListenable(directlyResolvedAgentPubkeys);
    final agentPubkeys = agentPubkeysWithChannelBots(
      knownAgentPubkeys: agentPubkeysWithProfileOwners(
        knownAgentPubkeys: {
          ...ref.watch(knownAgentPubkeysProvider),
          ...directAgentPubkeys,
        },
        profileOwnedAgentPubkeys: profileOwnedAgentPubkeys,
      ),
      channelBotPubkeys:
          channelBotPubkeysState.asData?.value ?? const <String>{},
    );
    useEffect(() {
      if (sessionStatus != SessionStatus.connected ||
          identitySubscriptionPubkeys.isEmpty) {
        return null;
      }
      var disposed = false;
      var subscriptionFailed = false;
      void markFailed() {
        subscriptionFailed = true;
        if (!disposed) identitySubscriptionReady.value = false;
      }

      void Function()? unsubscribe;
      Future.microtask(() async {
        try {
          final cleanup = await _subscribeToDmIdentityUpdates(
            ref,
            identitySubscriptionPubkeys,
            onReadyChanged: (isReady) {
              if (!disposed && !subscriptionFailed) {
                identitySubscriptionReady.value = isReady;
              }
            },
            onAgentPubkeysChanged: (pubkeys) {
              if (!disposed) directlyResolvedAgentPubkeys.value = pubkeys;
            },
            onFailure: markFailed,
          );
          if (disposed) {
            cleanup();
          } else {
            unsubscribe = cleanup;
          }
        } catch (error) {
          if (!disposed) {
            debugPrint('[DmIdentity] live subscription failed: $error');
            markFailed();
          }
        }
      });
      return () {
        disposed = true;
        unsubscribe?.call();
      };
    }, [sessionStatus, resolvedChannel.id, identitySubscriptionKey]);
    final isAgentIdentityUnresolved =
        isOneToOneDm &&
        (sessionStatus != SessionStatus.connected ||
            !isIdentitySubscriptionReady ||
            agentDirectoryState.isLoading ||
            agentDirectoryState.hasError ||
            agentOwnersState.isLoading ||
            agentOwnersState.hasError ||
            !channelMembershipUpdateState.isReady ||
            channelMembershipUpdateState.error != null ||
            channelBotPubkeysState.isLoading ||
            channelBotPubkeysState.hasError ||
            memberProfilesPreloadState.connectionState !=
                ConnectionState.done ||
            memberProfilesPreloadState.data != true);
    final goalsEnabled = resolvedChannel.goalsEnabled;
    final showsHuddleAction =
        showsComposer &&
        !isAgentIdentityUnresolved &&
        !_isOneToOneAgentDm(resolvedChannel, agentPubkeys);
    final messagesNotifier = ref.read(
      channelMessagesProvider(channel.id).notifier,
    );
    final isConnectionInProgress =
        sessionStatus == SessionStatus.connecting ||
        sessionStatus == SessionStatus.reconnecting;
    final showConnectionSkeleton = useState(false);
    final shouldDebounceConnectionSkeleton =
        isConnectionInProgress &&
        (resolvedChannel.isForum || messagesNotifier.hasLoadedMessages);
    useEffect(() {
      if (!shouldDebounceConnectionSkeleton) {
        showConnectionSkeleton.value = false;
        return null;
      }
      final timer = Timer(const Duration(seconds: 2), () {
        showConnectionSkeleton.value = true;
      });
      return timer.cancel;
    }, [shouldDebounceConnectionSkeleton]);
    final showInitialConnectionSkeleton =
        !resolvedChannel.isForum &&
        isConnectionInProgress &&
        !messagesNotifier.hasLoadedMessages;
    final appBarTitleContentHeight = _twoLineAppBarTitleContentHeight(
      context,
      isDm: resolvedChannel.isDm,
    );
    final usesNativeIosGlassBackButton =
        Navigator.canPop(context) &&
        Theme.of(context).platform == TargetPlatform.iOS;
    final readTimestamp = _channelReadTimestamp(
      channel: resolvedChannel,
      messagesState: messagesState,
      heldNewerEvents: messagesNotifier.hasNewer
          ? messagesNotifier.heldLiveEvents
          : null,
    );

    // The account this page opened under keeps its positions, even if the
    // account changes while the page is open.
    final roomPositions = useMemoized(
      () => channel.isForum ? null : ref.read(roomPositionStoreProvider),
    );
    final savedPosition = useMemoized(() => roomPositions?.read(channel.id));

    // A link or notification opens at its message, unread messages at the
    // first of them, and otherwise a saved position at its row, instead of
    // the newest row. The list waits for that window rather than drawing the
    // newest rows first.
    final openTarget = useMemoized(
      () => channel.isForum || initialThreadRootId != null
          ? null
          : initialMessageId != null
          ? _OpenTarget(ids: {initialMessageId!}, isUnread: false)
          : initialOldestOrdinaryUnreadMessageId != null ||
                initialForcedUnreadMessageIds.isNotEmpty
          ? _OpenTarget(
              ids: {
                ?initialOldestOrdinaryUnreadMessageId,
                ...initialForcedUnreadMessageIds,
              },
              isUnread: true,
              knownCreatedAt: {
                ?initialOldestOrdinaryUnreadMessageId:
                    initialOldestOrdinaryUnreadCreatedAtRef.value!,
              },
            )
          // Unread messages are only known once the read state is: until
          // then a saved position could hide them.
          : readState.isReady && savedPosition != null
          ? _OpenTarget(
              ids: {savedPosition.id},
              isUnread: false,
              knownCreatedAt: {savedPosition.id: savedPosition.createdAt},
              edge: savedPosition.edge,
            )
          : null,
    );
    final openPhase = useState(
      openTarget != null ||
              (messagesNotifier.isInMiddleWindow && messagesNotifier.hasNewer)
          ? _OpenPhase.opening
          : _OpenPhase.none,
    );
    final openedAt = useRef<_OpenedAt?>(null);
    final didStartOpening = useRef(false);
    // Without newer pages there is no window to open at: draw the newest rows
    // now rather than after the message is found.
    useEffect(() {
      if (openTarget == null || openPhase.value != _OpenPhase.opening) {
        return null;
      }
      unawaited(
        messagesNotifier.canOpenAt().catchError((Object _) => false).then((
          canOpen,
        ) {
          if (!canOpen &&
              context.mounted &&
              openPhase.value == _OpenPhase.opening) {
            didStartOpening.value = true;
            openPhase.value = _OpenPhase.none;
          }
        }),
      );
      return null;
    }, const []);
    useEffect(() {
      if (openPhase.value != _OpenPhase.opening || didStartOpening.value) {
        return null;
      }
      final Future<bool> opening;
      final target = openTarget;
      if (target == null) {
        // A window left open at a message by an earlier visit; this visit
        // has nothing to open at, so it starts at the newest rows.
        opening = messagesNotifier.jumpToLatest().then((_) => false);
      } else {
        final loaded = messagesState.value ?? const <NostrEvent>[];
        final anchors = <({String id, int createdAt})>[];
        for (final id in target.ids) {
          final createdAt =
              target.knownCreatedAt[id] ??
              (loaded.where((event) => event.id == id).firstOrNull ??
                      messagesNotifier.deepLinkEvent(id))
                  ?.createdAt;
          // Wait for the by-id load before giving up on a target.
          if (createdAt == null && deepLinkLoads.value == 0) return null;
          if (createdAt != null) anchors.add((id: id, createdAt: createdAt));
        }
        anchors.sort((a, b) => a.createdAt.compareTo(b.createdAt));
        final anchor = anchors.firstOrNull;
        if (anchor == null) {
          didStartOpening.value = true;
          openPhase.value = _OpenPhase.none;
          return null;
        }
        openedAt.value = _OpenedAt(
          messageId: anchor.id,
          createdAt: anchor.createdAt,
          isUnread: target.isUnread,
          edge: target.edge,
        );
        opening = messagesNotifier.openAt(
          createdAt: anchor.createdAt,
          eventId: anchor.id,
        );
      }
      didStartOpening.value = true;
      unawaited(
        opening.catchError((Object _) => false).then((opened) {
          if (!context.mounted) return;
          final saved = target?.edge != null ? openedAt.value : null;
          if (opened && saved != null) {
            // A saved row deleted since is not remembered any longer.
            final events = ref.read(channelMessagesProvider(channel.id)).value;
            if (events?.any((event) => event.id == saved.messageId) != true) {
              roomPositions?.clear(channel.id);
            }
          }
          openPhase.value = opened ? _OpenPhase.positioning : _OpenPhase.none;
        }),
      );
      return null;
    }, [deepLinkLoads.value, messagesState.value]);
    final isOpenedAtMessage =
        openPhase.value == _OpenPhase.positioning ||
        openPhase.value == _OpenPhase.opened;

    useEffect(() {
      final session = ref.read(relaySessionProvider.notifier);
      return session.registerVisibleChannel(channel.id);
    }, [channel.id]);

    useEffect(
      () {
        if (channel.isForum) return null;
        final eventIds = {
          ?initialMessageId,
          ?initialThreadRootId,
          ?initialOldestOrdinaryUnreadMessageId,
          ...initialForcedUnreadMessageIds,
        };
        if (eventIds.isEmpty) return null;
        final notifier = ref.read(channelMessagesProvider(channel.id).notifier);
        unawaited(
          _loadDeepLinkEvents(ref, channel.id, eventIds).then((_) {
            if (context.mounted) deepLinkLoads.value++;
          }),
        );
        return () => notifier.releaseDeepLinkEvents(eventIds);
      },
      [
        channel.id,
        initialMessageId,
        initialThreadRootId,
        initialOldestOrdinaryUnreadMessageId,
        initialForcedUnreadMessageIds,
      ],
    );

    // Start the thread's newest replies alongside its head instead of after
    // the thread page mounts, so opening it costs one relay round trip. The
    // thread page watches the same query and picks up the in-flight result.
    useEffect(() {
      final rootId = initialThreadRootId;
      if (channel.isForum || rootId == null) return null;
      final subscription = ref.listenManual(
        threadRepliesProvider(
          ThreadRepliesArgs(channelId: channel.id, rootId: rootId),
        ),
        (_, _) {},
      );
      return subscription.close;
    }, [channel.id, initialThreadRootId]);

    // A thread opened straight from Inbox, a draft, a goal or a notification
    // never shows the channel: this route stands in with a thread-shaped
    // placeholder and swaps itself for the thread once the head is known,
    // either cached on the timeline or fetched by id above.
    final directThreadEvents = messagesState.value ?? const <NostrEvent>[];
    final directThreadHead = opensThreadDirectly
        ? directThreadEvents
                  .where((event) => event.id == initialThreadRootId)
                  .firstOrNull ??
              ref
                  .read(channelMessagesProvider(channel.id).notifier)
                  .deepLinkEvent(initialThreadRootId!)
        : null;
    // The head could not be found (deleted, or the fetch failed) once both the
    // by-id fetch and the timeline have settled: show the channel instead.
    final directThreadMissing =
        opensThreadDirectly &&
        directThreadHead == null &&
        deepLinkLoads.value > 0 &&
        messagesState.hasValue &&
        !messagesState.isLoading;
    useEffect(() {
      final rootId = initialThreadRootId;
      final head = directThreadHead;
      if (rootId == null || head == null || didOpenDirectThread.value) {
        return null;
      }
      didOpenDirectThread.value = true;
      final events = directThreadEvents;
      final messages = formatTimeline([
        ...events,
        if (!events.any((event) => event.id == rootId)) head,
      ], currentPubkey: currentPubkey);
      final threadHead = messages.firstWhere((message) => message.id == rootId);
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!context.mounted || ModalRoute.of(context)?.isCurrent != true) {
          return;
        }
        Navigator.of(context).pushReplacement(
          _DirectThreadRoute(
            builder: (_) => ThreadDetailPage(
              threadHead: threadHead,
              allMessages: messages,
              channelId: channel.id,
              currentPubkey: currentPubkey,
              isMember: resolvedChannel.isMember,
              isArchived: resolvedChannel.isArchived,
              initialMessageId: initialMessageId,
              channelBelow: false,
              ownerView: ownerView,
            ),
          ),
        );
      });
      return null;
    }, [directThreadHead]);

    useEffect(() {
      // The owner view reads someone else's chat: nothing is marked read.
      if (ownerView != null || !readState.isReady || readTimestamp == null) {
        return null;
      }
      return deferReadStateUpdate(context, () {
        ref
            .read(readStateProvider.notifier)
            .markContextRead(channel.id, readTimestamp);
        ref
            .read(channelsProvider.notifier)
            .clearObservedUnreadCoveredByRead(channel.id, readTimestamp);
      });
    }, [channel.id, readState.isReady, readTimestamp]);

    if (opensThreadDirectly && !directThreadMissing) {
      return _DirectThreadPlaceholder(
        channelId: channel.id,
        rootId: initialThreadRootId!,
      );
    }

    final scaffold = FrostedScaffold(
      resizeToAvoidBottomInset:
          !usesFixedAndroidImeViewport || resolvedChannel.isForum,
      appBar: FrostedAppBar(
        gradient: ownerView == null
            ? null
            : LinearGradient(
                colors: [
                  ownerViewBackground(context),
                  ownerViewBackground(context),
                ],
              ),
        bottomHeight: appBarBottomHeight,
        bottom: appBarBottomHeight == 0
            ? null
            : Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  if (ownerReaders.isNotEmpty)
                    OwnerCanReadNotice(readers: ownerReaders),
                  if (isMyAgentDm)
                    Padding(
                      padding: const EdgeInsets.fromLTRB(
                        Grid.gutter,
                        0,
                        Grid.gutter,
                        Grid.xs,
                      ),
                      child: SizedBox(
                        height: _agentTabsHeight,
                        child: KuzzViewSwitch<AgentDmTab>(
                          semanticsLabel: context.l10n.ownerViewTabsLabel,
                          options: [
                            KuzzViewOption(
                              value: AgentDmTab.chat,
                              label: context.l10n.ownerViewTabChat(dmAgentName),
                              key: const ValueKey('agent-dm-tab-chat'),
                            ),
                            KuzzViewOption(
                              value: AgentDmTab.conversations,
                              label: [
                                context.l10n.ownerViewTabConversations(
                                  dmAgentName,
                                ),
                                if (agentDots > 0) '$agentDots',
                              ].join(' '),
                              key: const ValueKey('agent-dm-tab-conversations'),
                            ),
                          ],
                          selected: showsAgentConversations
                              ? AgentDmTab.conversations
                              : AgentDmTab.chat,
                          onChanged: (tab) => ref
                              .read(agentDmTabProvider(channel.id).notifier)
                              .set(tab),
                        ),
                      ),
                    ),
                ],
              ),
        leading: usesNativeIosGlassBackButton
            ? IosGlassNavigationButton(
                key: const ValueKey('channel-ios-glass-back'),
                icon: IosGlassNavigationIcon.back,
                semanticLabel: context.l10n.commonBack,
                onPressed: () => Navigator.of(context).maybePop(),
                width: iosGlassChannelHeaderLeadingWidth,
                buttonCenterX: iosGlassChannelHeaderButtonCenterX,
                nativeViewSuppressed: messageActionBackdropActive,
              )
            : null,
        iconColor: context.colors.primary,
        titleContentHeight: appBarTitleContentHeight,
        titleStyle: channelTitleTextStyle,
        title: Padding(
          padding: EdgeInsetsDirectional.only(
            start: usesNativeIosGlassBackButton
                ? iosGlassChannelHeaderTitleSpacing
                : 0,
          ),
          child: resolvedChannel.isDm
              ? _DmAppBarTitle(
                  channel: resolvedChannel,
                  currentPubkey: ownerView?.agentPubkey ?? currentPubkey,
                  ownerViewSubtitle: ownerView == null
                      ? null
                      : context.l10n.ownerViewReadOnlySubtitle(
                          ownerView.agentName,
                        ),
                )
              : _ChannelAppBarTitle(
                  channel: resolvedChannel,
                  ownerViewSubtitle: ownerView == null
                      ? null
                      : context.l10n.ownerViewReadOnlySubtitle(
                          ownerView.agentName,
                        ),
                  onTap: () async {
                    final shouldClose = await showChannelDetailsPage(
                      context: context,
                      channel: resolvedChannel,
                      currentPubkey: currentPubkey,
                      onMemberTap: (context, pubkey) => showUserProfileSheet(
                        context,
                        pubkey,
                        names: channelIdentityNamesProvider(resolvedChannel.id),
                      ),
                      sectionId: ref
                          .read(channelSectionsProvider)
                          .store
                          .assignments[resolvedChannel.id],
                    );
                    if (shouldClose == true && context.mounted) {
                      Navigator.of(context).pop();
                    }
                  },
                ),
        ),
        actions: ownerView != null
            ? const []
            : resolvedChannel.isDm
            ? [
                if (goalsEnabled) _GoalsButton(channel: resolvedChannel),
                if (showsHuddleAction)
                  _HuddleButton(
                    channel: resolvedChannel,
                    events: [
                      ...messagesState.value ?? const [],
                      ...huddleLifecycle,
                    ],
                  ),
                if (_showsMembersAction(resolvedChannel))
                  _MembersButton(
                    channelId: resolvedChannel.id,
                    channel: resolvedChannel,
                    currentPubkey: currentPubkey,
                  ),
                IconButton(
                  color: context.colors.primary,
                  onPressed: () async {
                    final shouldClose = await showChannelActionsSheet(
                      context: context,
                      channel: resolvedChannel,
                      isUnread: false,
                      sectionId: ref
                          .read(channelSectionsProvider)
                          .store
                          .assignments[resolvedChannel.id],
                    );
                    if (shouldClose == true && context.mounted) {
                      Navigator.of(context).pop();
                    }
                  },
                  tooltip: context.l10n.channelGroupActions,
                  icon: const Icon(LucideIcons.ellipsisVertical, size: 22),
                ),
              ]
            : [
                if (goalsEnabled && !resolvedChannel.isForum)
                  _GoalsButton(channel: resolvedChannel),
                if (showsComposer)
                  _HuddleButton(
                    channel: resolvedChannel,
                    events: [
                      ...messagesState.value ?? const [],
                      ...huddleLifecycle,
                    ],
                  ),
              ],
      ),
      body: _withTopInset(
        context,
        appBarBottomHeight,
        showsAgentConversations
            ? AgentConversationsList(
                agentPubkey: dmAgentPubkey,
                agentName: dmAgentName,
                dmChannelId: channel.id,
                topInset: frostedAppBarHeight(
                  context,
                  titleContentHeight: appBarTitleContentHeight,
                  bottomHeight: appBarBottomHeight,
                ),
              )
            : Stack(
                fit: StackFit.expand,
                children: [
                  Column(
                    children: [
                      Expanded(
                        child: resolvedChannel.isForum
                            ? Stack(
                                fit: StackFit.expand,
                                children: [
                                  ForumPostsView(
                                    channel: resolvedChannel,
                                    currentPubkey: currentPubkey,
                                  ),
                                  if (showConnectionSkeleton.value)
                                    Positioned(
                                      top:
                                          frostedAppBarHeight(
                                            context,
                                            titleContentHeight:
                                                appBarTitleContentHeight,
                                          ) +
                                          Grid.xs,
                                      left: Grid.gutter,
                                      right: Grid.gutter,
                                      child: _ForumConnectionSkeleton(
                                        status: sessionStatus,
                                      ),
                                    ),
                                ],
                              )
                            : SkeletonReveal(
                                loading:
                                    showInitialConnectionSkeleton ||
                                    showConnectionSkeleton.value ||
                                    messagesState.isLoading ||
                                    openPhase.value == _OpenPhase.opening ||
                                    openPhase.value == _OpenPhase.positioning,
                                shimmerEnabled:
                                    sessionStatus != SessionStatus.disconnected,
                                skeleton: _MessageTimelineSkeleton(
                                  appBarTitleContentHeight:
                                      appBarTitleContentHeight,
                                  status: sessionStatus,
                                ),
                                content: messagesState.when(
                                  loading: SizedBox.shrink,
                                  error: (e, _) => Padding(
                                    padding: EdgeInsets.only(
                                      top: frostedAppBarHeight(
                                        context,
                                        titleContentHeight:
                                            appBarTitleContentHeight,
                                      ),
                                    ),
                                    child: LoadErrorView(
                                      message:
                                          context.l10n.channelLoadMessagesError,
                                      onRetry: () => ref.invalidate(
                                        channelMessagesProvider(channel.id),
                                      ),
                                    ),
                                  ),
                                  data: (events) {
                                    if (openPhase.value == _OpenPhase.opening) {
                                      return const SizedBox.shrink();
                                    }
                                    final messages = formatTimeline(
                                      events,
                                      currentPubkey: currentPubkey,
                                    );
                                    final summaries = ref
                                        .read(
                                          channelMessagesProvider(
                                            channel.id,
                                          ).notifier,
                                        )
                                        .threadSummaries;
                                    final entries = buildMainTimelineEntries(
                                      messages,
                                      relaySummaries: summaries,
                                    );
                                    return _MessageList(
                                      entries: entries,
                                      allMessages: messages,
                                      initialMessageId: initialMessageId,
                                      initialThreadRootId: initialThreadRootId,
                                      initialThreadRouteBehavior:
                                          initialThreadRouteBehavior,
                                      highlightInitialMessage:
                                          highlightInitialMessage,
                                      initialOrdinaryUnreadMessageIds:
                                          initialOrdinaryUnreadMessageIds,
                                      initialOldestOrdinaryUnreadMessageId:
                                          initialOldestOrdinaryUnreadMessageId,
                                      initialForcedUnreadMessageIds:
                                          initialForcedUnreadMessageIds,
                                      openedAt: isOpenedAtMessage
                                          ? openedAt.value
                                          : null,
                                      isPositioning:
                                          openPhase.value ==
                                          _OpenPhase.positioning,
                                      onPositioned: () {
                                        if (openPhase.value ==
                                            _OpenPhase.positioning) {
                                          openPhase.value = _OpenPhase.opened;
                                        }
                                      },
                                      roomPositions: roomPositions,
                                      hasInitialUnread:
                                          !isOpenedAtMessage &&
                                          readState.isReady &&
                                          (readState.isForcedUnread(
                                                channel.id,
                                              ) ||
                                              initialForcedUnreadMessageIds
                                                  .isNotEmpty ||
                                              initialOldestOrdinaryUnreadMessageId !=
                                                  null),
                                      channelId: channel.id,
                                      currentPubkey: currentPubkey,
                                      isMember: resolvedChannel.isMember,
                                      isArchived: resolvedChannel.isArchived,
                                      appBarTitleContentHeight:
                                          appBarTitleContentHeight,
                                      composerBottomInset: showsComposer
                                          ? composerDockHeight.value
                                          : 0,
                                      composerFocusNode: showsComposeBar
                                          ? composerFocusNode
                                          : null,
                                      restoreComposerFocus: showsComposeBar
                                          ? () => restoreComposerFocus.value
                                                ?.call()
                                          : null,
                                      quoteScope: showsComposeBar
                                          ? quoteScope
                                          : null,
                                    );
                                  },
                                ),
                              ),
                      ),
                      if (!resolvedChannel.isForum &&
                          (!resolvedChannel.isMember ||
                              resolvedChannel.isArchived)) ...[
                        AnimatedSize(
                          duration: MediaQuery.disableAnimationsOf(context)
                              ? Duration.zero
                              : const Duration(milliseconds: 180),
                          curve: Curves.easeOutCubic,
                          alignment: Alignment.bottomCenter,
                          child: visibleTypingEntries.isEmpty
                              ? const SizedBox.shrink()
                              : ChannelTypingIndicator(
                                  channelId: resolvedChannel.id,
                                  entries: visibleTypingEntries,
                                ),
                        ),
                        if (ownerView != null)
                          OwnerViewBar(
                            view: ownerView,
                            channelId: resolvedChannel.id,
                          )
                        else if (resolvedChannel.canJoin)
                          GroupJoinBar(channel: resolvedChannel)
                        else if (!resolvedChannel.isPersonChat)
                          _ReadOnlyNotice(channel: resolvedChannel),
                      ],
                    ],
                  ),
                  if (showsComposer)
                    AndroidImeLift(
                      child: Align(
                        alignment: Alignment.bottomCenter,
                        child: ComposerDockSizeReporter(
                          key: const ValueKey('channel-composer-dock'),
                          onHeightChanged: (height) {
                            if ((composerDockHeight.value - height).abs() <
                                0.5) {
                              return;
                            }
                            composerDockHeight.value = height;
                          },
                          child: Column(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              AnimatedSize(
                                duration:
                                    MediaQuery.disableAnimationsOf(context)
                                    ? Duration.zero
                                    : const Duration(milliseconds: 180),
                                curve: Curves.easeOutCubic,
                                alignment: Alignment.bottomCenter,
                                child: visibleTypingEntries.isEmpty
                                    ? const SizedBox.shrink()
                                    : ChannelTypingIndicator(
                                        channelId: resolvedChannel.id,
                                        entries: visibleTypingEntries,
                                      ),
                              ),
                              if (blockedCounterpart != null)
                                BlockedDmBar(pubkey: blockedCounterpart)
                              else ...[
                                ComposerQuoteChip(scope: quoteScope),
                                ComposeBar(
                                  channelId: channel.id,
                                  channelMainAgentAudience:
                                      !resolvedChannel.isDm,
                                  focusNode: composerFocusNode,
                                  onFocusRestorerChanged: (restoreFocus) =>
                                      restoreComposerFocus.value = restoreFocus,
                                  channelName: resolvedChannel.isPersonChat
                                      ? ''
                                      : resolvedChannel.name,
                                  onSend:
                                      (
                                        content,
                                        mentionPubkeys, {
                                        mediaTags = const <List<String>>[],
                                      }) async {
                                        // Sending returns the reader to the newest row.
                                        roomPositions?.clear(channel.id);
                                        // Main timeline: top-level message (no thread
                                        // tags) plus the optional quote reference.
                                        await sendMessage.call(
                                          channelId: channel.id,
                                          content: content,
                                          mentionPubkeys: mentionPubkeys,
                                          channel: resolvedChannel,
                                          mediaTags: mediaTags,
                                          quote: pendingQuote,
                                          keepFailedMessage: true,
                                        );
                                        if (pendingQuote != null &&
                                            context.mounted) {
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
                            ],
                          ),
                        ),
                      ),
                    ),
                ],
              ),
      ),
    );
    return ownerView == null
        ? scaffold
        : OwnerViewScope(view: ownerView, child: scaffold);
  }
}

const _agentTabsHeight = 36.0;

/// [child] laid out as if the header were [extra] taller: the timeline's
/// spacing under the header follows the safe area's top.
Widget _withTopInset(BuildContext context, double extra, Widget child) {
  if (extra == 0) return child;
  final media = MediaQuery.of(context);
  return MediaQuery(
    data: media.copyWith(
      padding: media.padding.copyWith(top: media.padding.top + extra),
    ),
    child: child,
  );
}
