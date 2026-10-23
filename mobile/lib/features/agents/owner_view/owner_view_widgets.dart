import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../../shared/l10n/l10n.dart';
import '../../../shared/mentions/agent_identity_provider.dart';
import '../../../shared/profile/user_cache_provider.dart';
import '../../../shared/theme/theme.dart';
import '../../../shared/utils/string_utils.dart';
import '../../../shared/widgets/modal_presentation.dart';
import '../../activity/compose_drafts_provider.dart';
import '../../channels/channel_detail_page.dart';
import '../../channels/channel_management_provider.dart';
import '../my_agent_actions.dart';
import 'owner_view.dart';

/// Stands in for the composer in a chat read in the owner view: nothing can
/// be written here. Ask goes to the DM with the agent with a draft about
/// this chat; Remove takes the agent out of the chat (the relay lets an
/// agent's owner do that) and returns to the agent's conversations.
class OwnerViewBar extends HookConsumerWidget {
  const OwnerViewBar({super.key, required this.view, required this.channelId});

  final OwnerView view;
  final String channelId;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final busy = useState(false);
    final name = view.agentName;
    final chatLabel = view.chatLabel;

    Future<void> ask() async {
      final navigator = Navigator.of(context);
      final messenger = ScaffoldMessenger.of(context);
      final l10n = context.l10n;
      final drafts = ref.read(composeDraftsProvider.notifier);
      final key = composeDraftKey(view.dmChannelId);
      final prompt = l10n.ownerViewAskDraft(chatLabel);
      final existing = drafts.textFor(key)?.trimRight() ?? '';
      drafts.save(
        key: key,
        channelId: view.dmChannelId,
        text: existing.isEmpty ? prompt : '$existing\n\n$prompt',
      );
      ref
          .read(agentDmTabProvider(view.dmChannelId).notifier)
          .set(AgentDmTab.chat);
      final route = openChannelRoute(view.dmChannelId);
      if (route != null) {
        navigator.popUntil((candidate) => candidate == route);
        return;
      }
      try {
        final dm = await ref.read(myAgentDmProvider)(view.agentPubkey);
        await navigator.push(
          MaterialPageRoute<void>(
            builder: (_) => ChannelDetailPage(channel: dm),
          ),
        );
      } catch (failure) {
        messenger.showSnackBar(
          SnackBar(content: Text(l10n.agentsOpenDmError(name, '$failure'))),
        );
      }
    }

    Future<void> remove() async {
      final confirmed = await showKuzzDialog<bool>(
        context: context,
        builder: (dialogContext) => AlertDialog(
          title: Text(
            dialogContext.l10n.ownerViewRemoveConfirmTitle(name, chatLabel),
          ),
          content: Text(dialogContext.l10n.ownerViewRemoveConfirmBody(name)),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(dialogContext).pop(false),
              child: Text(dialogContext.l10n.commonCancel),
            ),
            FilledButton(
              key: const ValueKey('owner-view-remove-confirm'),
              onPressed: () => Navigator.of(dialogContext).pop(true),
              child: Text(dialogContext.l10n.ownerViewRemoveConfirmAction),
            ),
          ],
        ),
      );
      if (confirmed != true || !context.mounted) return;
      final navigator = Navigator.of(context);
      final messenger = ScaffoldMessenger.of(context);
      final l10n = context.l10n;
      final container = ProviderScope.containerOf(context, listen: false);
      busy.value = true;
      try {
        await ref
            .read(channelActionsProvider)
            .removeMember(channelId: channelId, pubkey: view.agentPubkey);
      } catch (error) {
        if (!isMemberAlreadyGoneError(error)) {
          messenger.showSnackBar(
            SnackBar(content: Text(l10n.ownerViewRemoveFailed(name))),
          );
          if (context.mounted) busy.value = false;
          return;
        }
      }
      container.invalidate(agentChatListProvider(view.agentPubkey));
      messenger.showSnackBar(
        SnackBar(content: Text(l10n.ownerViewRemoved(name))),
      );
      final route = openChannelRoute(view.dmChannelId);
      if (route != null) {
        navigator.popUntil((candidate) => candidate == route);
      } else {
        unawaited(navigator.maybePop());
      }
    }

    Widget label(String text) => Text(
      text,
      maxLines: 2,
      overflow: TextOverflow.ellipsis,
      textAlign: TextAlign.center,
    );

    return Container(
      key: const ValueKey('owner-view-bar'),
      width: double.infinity,
      padding: EdgeInsets.fromLTRB(
        Grid.gutter,
        Grid.xxs,
        Grid.gutter,
        MediaQuery.viewPaddingOf(context).bottom + Grid.xxs,
      ),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: context.colors.outlineVariant)),
        color: ownerViewBackground(context),
      ),
      child: Row(
        children: [
          Expanded(
            child: OutlinedButton.icon(
              key: const ValueKey('owner-view-ask'),
              onPressed: busy.value ? null : () => unawaited(ask()),
              icon: const Icon(LucideIcons.quote, size: 16),
              label: label(context.l10n.ownerViewAsk(name)),
            ),
          ),
          const SizedBox(width: Grid.xxs),
          Expanded(
            child: OutlinedButton.icon(
              key: const ValueKey('owner-view-remove'),
              style: OutlinedButton.styleFrom(
                foregroundColor: context.colors.error,
              ),
              onPressed: busy.value ? null : () => unawaited(remove()),
              icon: const Icon(LucideIcons.logOut, size: 16),
              label: label(context.l10n.ownerViewRemove(name)),
            ),
          ),
        ],
      ),
    );
  }
}

/// "akak (Fizz's owner) can read this chat": one line per agent in the chat
/// whose owner is not in it, for the people who are.
class OwnerCanReadNotice extends HookConsumerWidget {
  const OwnerCanReadNotice({super.key, required this.readers});

  final List<OwnerReader> readers;

  /// Height of one line; the header reserves this per reader.
  static const lineHeight = 22.0;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final pubkeys = {
      for (final reader in readers) ...[reader.agentPubkey, reader.ownerPubkey],
    }.toList();
    useEffect(() {
      final missing = [
        for (final pubkey in pubkeys)
          if (ref.read(userCacheProvider)[pubkey] == null) pubkey,
      ];
      if (missing.isNotEmpty) {
        Future.microtask(
          () => ref.read(userCacheProvider.notifier).preload(missing),
        );
      }
      return null;
    }, [pubkeys.join(',')]);
    final directory = ref.watch(agentDirectoryDisplayNamesProvider);
    String nameOf(String pubkey) {
      final name = ref.watch(
        userCacheProvider.select((cache) => cache[pubkey]?.displayName),
      );
      if (name != null && name.trim().isNotEmpty) return name.trim();
      return directory[pubkey] ?? shortPubkey(pubkey);
    }

    final style = context.textTheme.bodySmall?.copyWith(
      color: context.colors.onSurfaceVariant,
    );
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final reader in readers)
          SizedBox(
            key: ValueKey('owner-can-read-notice-${reader.agentPubkey}'),
            height: lineHeight,
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
              child: Row(
                children: [
                  Icon(
                    LucideIcons.eye,
                    size: 13,
                    color: context.colors.onSurfaceVariant,
                  ),
                  const SizedBox(width: Grid.half),
                  Expanded(
                    child: Text(
                      context.l10n.ownerViewNotice(
                        nameOf(reader.ownerPubkey),
                        nameOf(reader.agentPubkey),
                      ),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: style,
                    ),
                  ),
                ],
              ),
            ),
          ),
      ],
    );
  }
}
