import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/default_community.dart';
import '../../shared/deeplink/deep_link.dart';
import '../../shared/deeplink/pending_deep_link_provider.dart';
import '../../shared/l10n/l10n.dart';
import '../groups/group_preview.dart';
import '../invites/invite_join_provider.dart';
import '../invites/invite_join_sheet.dart';
import 'channel.dart';
import 'channel_detail_page.dart';
import 'channels_provider.dart';
import '../chats/chat_list_provider.dart';
import '../chats/chat_row_channel.dart';

/// Routes pending `kuzz://message` deep links into the channel view.
///
/// Wraps the authenticated home subtree. Whenever a parsed link is parked in
/// [pendingDeepLinkProvider] and the channel list is available, this pushes
/// the target [ChannelDetailPage] on the enclosing [Navigator]. A room the
/// channel list (saved or the relay's) or a Chats row knows opens at once;
/// other links are held (not dropped) until the relay's list arrives, and
/// only a room missing from it is reported as not found.
typedef DeepLinkDestinationBuilder =
    Widget Function(Channel channel, KuzzDeepLink link);

class DeepLinkDispatcher extends ConsumerStatefulWidget {
  final Widget child;
  final DeepLinkDestinationBuilder? destinationBuilder;
  final bool dispatchMessageLinks;

  const DeepLinkDispatcher({
    super.key,
    required this.child,
    this.destinationBuilder,
    this.dispatchMessageLinks = true,
  });

  @override
  ConsumerState<DeepLinkDispatcher> createState() => _DeepLinkDispatcherState();
}

class _DeepLinkDispatcherState extends ConsumerState<DeepLinkDispatcher> {
  bool _preparingInvite = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _maybeDispatch(ref.read(pendingDeepLinkProvider));
    });
  }

  @override
  Widget build(BuildContext context) {
    // Re-evaluate dispatch when either a new link arrives or channels load.
    ref.listen<KuzzDeepLink?>(pendingDeepLinkProvider, (_, link) {
      _maybeDispatch(link);
    });
    if (widget.dispatchMessageLinks) {
      ref.listen<AsyncValue<List<Channel>>>(channelsProvider, (_, _) {
        _maybeDispatch(ref.read(pendingDeepLinkProvider));
      });
    }

    return widget.child;
  }

  void _maybeDispatch(KuzzDeepLink? link) {
    if (link == null || _preparingInvite) return;
    if (link is InviteDeepLink) {
      _maybeDispatchInvite(link);
      return;
    }
    if (link is GroupJoinDeepLink) {
      // Held while signed out; the signed-in dispatcher opens it.
      if (!widget.dispatchMessageLinks) return;
      // Open Join with link on the target; its preview does the rest.
      ref.read(pendingDeepLinkProvider.notifier).consume();
      final target = link.target;
      unawaited(
        Future<void>.microtask(() async {
          if (!mounted) return;
          await showJoinWithLinkSheet(context, target: target);
        }),
      );
      return;
    }
    if ((link is! MessageDeepLink && link is! ChannelDeepLink) ||
        !widget.dispatchMessageLinks) {
      return;
    }
    if (link is MessageDeepLink) {
      unawaited(_dispatchNotificationLink(link));
      return;
    }

    _dispatchNavigableLink(link);
  }

  Future<void> _dispatchNotificationLink(MessageDeepLink link) async {
    final preparation = await ref
        .read(pendingDeepLinkProvider.notifier)
        .prepareCommunity(link);
    if (!mounted || ref.read(pendingDeepLinkProvider) != link) return;
    switch (preparation) {
      case DeepLinkCommunityPreparation.ready:
        _dispatchNavigableLink(link);
      case DeepLinkCommunityPreparation.switched:
        // The community-scoped app subtree remounts and consumes the parked
        // target after its channels load.
        return;
      case DeepLinkCommunityPreparation.unavailable:
        ref.read(pendingDeepLinkProvider.notifier).consume();
        ScaffoldMessenger.maybeOf(context)?.showSnackBar(
          SnackBar(
            content: Text(context.l10n.channelNotificationCommunityGone),
          ),
        );
      case DeepLinkCommunityPreparation.failed:
        ScaffoldMessenger.maybeOf(context)?.showSnackBar(
          SnackBar(
            content: Text(context.l10n.channelNotificationCommunityError),
          ),
        );
    }
  }

  void _dispatchNavigableLink(KuzzDeepLink link) {
    final channelId = switch (link) {
      MessageDeepLink(:final channelId) => channelId,
      ChannelDeepLink(:final channelId) => channelId,
      _ => throw StateError('unsupported navigable deep link: $link'),
    };
    final channels = ref.read(channelsProvider).asData?.value;
    // The listed room, else the room its Chats row describes (when the
    // Chats list is already up; it is not started here): either opens at
    // once, before the relay's channel list.
    final channel =
        channels?.where((c) => c.id == channelId).firstOrNull ??
        (ref.exists(chatListProvider)
            ? channelFromChatRows(
                ref.read(chatListProvider).value?.rows ?? const [],
                channelId,
              )
            : null);
    if (channel == null) {
      // Not known yet (no list, or only the saved one) — keep the link
      // parked; the channelsProvider listener re-attempts as lists arrive.
      if (channels == null ||
          ref.read(channelsProvider.notifier).showsSavedList) {
        return;
      }
      ref.read(pendingDeepLinkProvider.notifier).consume();
      debugPrint(
        'deep-link: channel $channelId not found in workspace; dropping link',
      );
      ScaffoldMessenger.maybeOf(context)?.showSnackBar(
        SnackBar(content: Text(context.l10n.channelLinkGroupNotFound)),
      );
      return;
    }
    if (!context.mounted) return;

    _pushChannel(channel, link);
    ref.read(pendingDeepLinkProvider.notifier).consume();
  }

  void _pushChannel(Channel channel, KuzzDeepLink link) {
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) =>
            widget.destinationBuilder?.call(channel, link) ??
            ChannelDetailPage(
              channel: channel,
              initialMessageId: link is MessageDeepLink ? link.messageId : null,
              initialThreadRootId: link is MessageDeepLink
                  ? link.threadRootId
                  : null,
              // A notification for a thread reply opens the thread itself;
              // Back returns to where the app was, not to the channel.
              initialThreadRouteBehavior:
                  InitialThreadRouteBehavior.replaceCurrentRoute,
            ),
      ),
    );
  }

  void _maybeDispatchInvite(InviteDeepLink link) {
    if (_preparingInvite) return;
    _preparingInvite = true;
    final navigatorContext = context;
    final messenger = ScaffoldMessenger.maybeOf(context);
    Future.microtask(() async {
      var consumed = false;
      try {
        await ref.read(inviteJoinProvider.notifier).prepare(link);
        ref.read(pendingDeepLinkProvider.notifier).consume();
        consumed = true;
        if (!navigatorContext.mounted) return;
        final inviteState = ref.read(inviteJoinProvider);
        final status = inviteState.status;
        if (status == InviteJoinStatus.confirming ||
            inviteState.isStarterSetupRecovery) {
          final sheet = showInviteJoinSheet(navigatorContext);
          if (status == InviteJoinStatus.claiming &&
              inviteState.isStarterSetupRecovery) {
            unawaited(
              ref.read(inviteJoinProvider.notifier).startStarterSetupRecovery(),
            );
          }
          final shouldFocusStarter = await sheet;
          final focusChannelId = ref.read(inviteJoinProvider).focusChannelId;
          if (shouldFocusStarter == true &&
              focusChannelId != null &&
              ref.read(pendingDeepLinkProvider) == null &&
              navigatorContext.mounted) {
            final channels = ref.read(channelsProvider).asData?.value;
            final channel = channels
                ?.where((candidate) => candidate.id == focusChannelId)
                .firstOrNull;
            if (channel != null) {
              _pushChannel(channel, ChannelDeepLink(channelId: focusChannelId));
            }
          }
        } else if (inviteState.differentService) {
          messenger?.showSnackBar(
            SnackBar(content: Text(appL10n.inviteDifferentService)),
          );
        } else if (status == InviteJoinStatus.switchedExisting) {
          messenger?.showSnackBar(
            SnackBar(
              content: Text(
                ref.read(hasBuiltInServerProvider)
                    ? appL10n.channelInviteAlreadyMember
                    : appL10n.channelInviteSwitchedCommunity,
              ),
            ),
          );
        }
      } catch (error) {
        debugPrint('deep-link: failed to prepare invite: $error');
        if (navigatorContext.mounted) {
          messenger?.showSnackBar(
            SnackBar(content: Text(appL10n.channelInviteOpenError)),
          );
        }
      } finally {
        _preparingInvite = false;
        if (mounted && consumed) {
          _maybeDispatch(ref.read(pendingDeepLinkProvider));
        }
      }
    });
  }
}
