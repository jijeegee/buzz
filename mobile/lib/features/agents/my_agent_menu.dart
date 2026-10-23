import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/devices/agent_host_devices.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/avatar_image.dart';
import '../../shared/widgets/modal_presentation.dart';
import '../profile/presence_cache_provider.dart';
import '../profile/user_profile_sheet.dart';
import 'my_agent_actions.dart';
import 'my_agents.dart';

/// The "⋮" on an agent: opens [showMyAgentSheet], the same sheet a long
/// press on the row opens.
class MyAgentMenu extends ConsumerWidget {
  const MyAgentMenu({super.key, required this.agent, required this.name});

  final MyAgent agent;
  final String name;

  @override
  Widget build(BuildContext context, WidgetRef ref) => IconButton(
    key: ValueKey('my-agent-menu-${agent.pubkey}'),
    icon: const Icon(LucideIcons.ellipsisVertical, size: 18),
    tooltip: context.l10n.agentsActions,
    onPressed: () => showMyAgentSheet(context, ref, agent: agent, name: name),
  );
}

/// What can be done with one of the owner's agents: Message and Start or
/// Stop up top, then Edit and View profile. Delete stays in its settings.
/// The actions run with [ref], the opening page's, since they outlive the
/// sheet.
Future<void> showMyAgentSheet(
  BuildContext context,
  WidgetRef ref, {
  required MyAgent agent,
  required String name,
}) => showKuzzModalBottomSheet<void>(
  context: context,
  isScrollControlled: true,
  showDragHandle: true,
  constraints: const BoxConstraints(maxWidth: 640),
  builder: (_) => MyAgentSheet(agent: agent, name: name, pageRef: ref),
);

class MyAgentSheet extends ConsumerWidget {
  const MyAgentSheet({
    super.key,
    required this.agent,
    required this.name,
    required this.pageRef,
  });

  final MyAgent agent;
  final String name;
  final WidgetRef pageRef;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final pubkey = agent.pubkey;
    final profile = ref.watch(
      userCacheProvider.select((cache) => cache[pubkey]),
    );
    final status = myAgentStatus(
      ref.watch(presenceCacheProvider.select((p) => p[pubkey])),
    );
    final busy = ref.watch(
      myAgentCommandsProvider.select((c) => c.containsKey(pubkey)),
    );
    final computer = myAgentComputerLabel(
      ref.watch(agentHostDeviceNamesProvider)[agent.deviceId],
      current: ref.watch(currentAccountDeviceIdProvider) == agent.deviceId,
    );
    final canStop = myAgentCanStop(agent, status);
    final initial = name.trim().isNotEmpty
        ? String.fromCharCode(name.trim().runes.first).toUpperCase()
        : '?';

    // Each action runs on the page under the sheet, after the sheet closes.
    void run(void Function(BuildContext page) action) {
      final page = Navigator.of(context, rootNavigator: true).context;
      Navigator.of(context).pop();
      action(page);
    }

    return SafeArea(
      key: ValueKey('my-agent-sheet-$pubkey'),
      top: false,
      child: IconTheme.merge(
        data: const IconThemeData(size: 22),
        child: SingleChildScrollView(
          padding: const EdgeInsets.fromLTRB(
            Grid.gutter,
            0,
            Grid.gutter,
            Grid.xs,
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  AvatarImage(
                    imageUrl: profile?.avatarUrl,
                    radius: activityAvatarSize / 2,
                    backgroundColor: context.colors.primaryContainer,
                    fallback: Text(
                      initial,
                      style: TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                        color: context.colors.onPrimaryContainer,
                      ),
                    ),
                    isAgent: true,
                  ),
                  const SizedBox(width: messageAvatarContentGap),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: activityUsernameTextStyle.copyWith(
                            color: context.colors.onSurface,
                          ),
                        ),
                        Text(
                          context.l10n.agentsStatusOnComputer(
                            myAgentStatusLabel(status),
                            computer,
                          ),
                          key: ValueKey('my-agent-sheet-status-$pubkey'),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: activityTimestampTextStyle.copyWith(
                            color: context.colors.onSurfaceVariant,
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
              const SizedBox(height: Grid.xs),
              Row(
                children: [
                  Expanded(
                    child: _QuickAction(
                      key: ValueKey('my-agent-menu-message-$pubkey'),
                      icon: LucideIcons.messageSquare,
                      label: context.l10n.agentsMessage,
                      onTap: () => run(
                        (page) => unawaited(
                          openMyAgentDm(page, pageRef, pubkey, name),
                        ),
                      ),
                    ),
                  ),
                  const SizedBox(width: Grid.twelve),
                  Expanded(
                    child: canStop
                        ? _QuickAction(
                            key: ValueKey('my-agent-menu-stop-$pubkey'),
                            icon: LucideIcons.square,
                            label: context.l10n.agentsStop,
                            onTap: busy
                                ? null
                                : () => run(
                                    (page) => unawaited(
                                      stopMyAgent(page, pageRef, agent, name),
                                    ),
                                  ),
                          )
                        : _QuickAction(
                            key: ValueKey('my-agent-menu-start-$pubkey'),
                            icon: LucideIcons.play,
                            label: context.l10n.agentsStart,
                            onTap: busy || !myAgentCanStart(agent, status)
                                ? null
                                : () => run(
                                    (page) => unawaited(
                                      startMyAgent(page, pageRef, agent, name),
                                    ),
                                  ),
                          ),
                  ),
                ],
              ),
              const SizedBox(height: Grid.xs),
              ListTile(
                key: ValueKey('my-agent-menu-edit-$pubkey'),
                contentPadding: EdgeInsets.zero,
                leading: const Icon(LucideIcons.pencil),
                title: Text(context.l10n.agentsEdit),
                onTap: () =>
                    run((page) => openMyAgentSettings(page, pubkey, name)),
              ),
              ListTile(
                key: ValueKey('my-agent-menu-profile-$pubkey'),
                contentPadding: EdgeInsets.zero,
                leading: const Icon(LucideIcons.userRound),
                title: Text(context.l10n.agentsViewProfile),
                onTap: () => run((page) => showUserProfileSheet(page, pubkey)),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A big square button in the sheet's top row; dimmed when [onTap] is null.
class _QuickAction extends StatelessWidget {
  const _QuickAction({
    super.key,
    required this.icon,
    required this.label,
    required this.onTap,
  });

  final IconData icon;
  final String label;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final color = onTap == null
        ? context.colors.onSurface.withValues(alpha: 0.38)
        : context.colors.onSurface;
    return InkWell(
      borderRadius: BorderRadius.circular(Radii.dialog),
      onTap: onTap == null
          ? null
          : () {
              unawaited(HapticFeedback.lightImpact());
              onTap?.call();
            },
      child: Container(
        height: 68 + (Grid.xxs * 2),
        decoration: BoxDecoration(
          color: context.colors.surfaceContainerHighest,
          borderRadius: BorderRadius.circular(Radii.dialog),
        ),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(icon, size: 22, color: color),
            const SizedBox(height: Grid.xxs),
            Text(
              label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: context.textTheme.labelMedium?.copyWith(color: color),
            ),
          ],
        ),
      ),
    );
  }
}
