import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:flutter/services.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/clipboard_utils.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/mentions/agent_identity_provider.dart';
import '../../shared/theme/theme.dart';
import '../../shared/relay/relay_provider.dart';
import '../../shared/widgets/app_list.dart';
import '../../shared/widgets/app_list_card.dart';
import '../../shared/widgets/avatar_image.dart';
import '../../shared/widgets/kuzz_action_tile.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/widgets/frosted_app_bar.dart';
import '../../shared/widgets/frosted_scaffold.dart';
import '../../shared/widgets/ios_glass_navigation_button.dart';
import '../../shared/widgets/lucide_star_icon.dart';
import '../../shared/widgets/modal_presentation.dart';
import '../../shared/widgets/sheet_divider.dart';
import '../groups/group_settings_section.dart';
import 'channel.dart';
import 'group_avatar.dart';
import 'group_photo_viewer.dart';
import 'group_chat/chat_info_sheet.dart';
import 'group_chat/start_chat_from_dm.dart';
import 'add_members_sheet.dart';
import 'channel_identity_names_provider.dart';
import 'channel_management_provider.dart';
import 'channel_mutes/channel_mutes_provider.dart';
import 'channel_sections/channel_sections_provider.dart';
import 'channel_stars/channel_stars_provider.dart';
import 'channels_provider.dart';
import 'manage_channel_sheet.dart';
import 'members_sheet.dart';
import 'room_goals_switch.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/read_state/read_state_provider.dart';
import '../../shared/read_state/read_state_time.dart';

export 'group_chat/chat_info_sheet.dart'
    show chatPeopleLabel, showChatInfoSheet;
export 'group_chat/chat_title.dart' show watchChannelTitle;

part 'channel_details_page.dart';

/// Opens the mobile channel actions sheet and returns whether its parent page
/// should close after a successful lifecycle action.
Future<bool?> showChannelActionsSheet({
  required BuildContext context,
  required Channel channel,
  required bool isUnread,
  VoidCallback? onMarkRead,
  String? sectionId,
}) => showKuzzModalBottomSheet<bool>(
  context: context,
  isScrollControlled: true,
  showDragHandle: true,
  constraints: BoxConstraints(
    maxWidth: 640,
    maxHeight: MediaQuery.sizeOf(context).height * 0.7,
  ),
  builder: (_) => ChannelActionsSheet(
    channel: channel,
    isUnread: isUnread,
    onMarkRead: onMarkRead,
    sectionId: sectionId,
  ),
);

/// Mobile action sheet for channel-level read, organization, and lifecycle
/// operations.
class ChannelActionsSheet extends HookConsumerWidget {
  const ChannelActionsSheet({
    super.key,
    required this.channel,
    required this.isUnread,
    this.onMarkRead,
    this.sectionId,
  });

  final Channel channel;
  final bool isUnread;
  final VoidCallback? onMarkRead;
  final String? sectionId;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final pinScope = useMemoized(
      () => (ref.read(relayConfigProvider).baseUrl, ref.read(myPubkeyProvider)),
      const [],
    );
    bool samePinScope() =>
        pinScope ==
        (ref.read(relayConfigProvider).baseUrl, ref.read(myPubkeyProvider));
    final displayedChannel = useState(channel);
    final currentChannel = displayedChannel.value;
    final isMuted =
        ref.watch(channelMutesProvider).store.channels[channel.id]?.muted ==
        true;
    final stars = ref.watch(channelStarsProvider);
    final isStarred = stars.store.channels[channel.id]?.starred == true;
    final membersAsync = channel.isPersonChat
        ? const AsyncValue<List<ChannelMember>>.data([])
        : ref.watch(channelMembersProvider(channel.id));
    final agentOwnersAsync = channel.isPersonChat
        ? const AsyncValue<Map<String, String>>.data({})
        : ref.watch(agentOwnersProvider);
    final currentPubkey = ref.watch(currentPubkeyProvider)?.toLowerCase();
    final currentMember = membersAsync.value?.cast<ChannelMember?>().firstWhere(
      (member) => member?.pubkey.toLowerCase() == currentPubkey,
      orElse: () => null,
    );
    final ownsOwnerAgent =
        currentPubkey != null &&
        membersAsync.value?.any(
              (member) =>
                  member.isOwner &&
                  agentOwnersAsync.value?[member.pubkey.toLowerCase()]
                          ?.toLowerCase() ==
                      currentPubkey,
            ) ==
            true;
    final canManageLifecycle =
        currentMember?.isElevated == true || ownsOwnerAgent;
    final canArchive = !channel.isArchived && canManageLifecycle;
    final canUnarchive = channel.isArchived && canManageLifecycle;
    final canDelete =
        !channel.isArchived &&
        (currentMember?.isOwner == true || ownsOwnerAgent);
    final lifecycleCapabilitiesLoading =
        membersAsync.isLoading || agentOwnersAsync.isLoading;
    final lifecycleCapabilitiesUnavailable =
        membersAsync.hasError || agentOwnersAsync.hasError;
    final dmOther = dmOtherPerson(currentChannel, currentPubkey: currentPubkey);
    // Any DM participant may change the DM's goals switch.
    final isDmParticipant =
        currentChannel.isMember ||
        (currentPubkey != null &&
            currentChannel.participantPubkeys.any(
              (pubkey) => pubkey.toLowerCase() == currentPubkey,
            ));

    void close() => Navigator.of(context).pop();

    return SafeArea(
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
            children: [
              ...[
                _ChannelQuickActionsRow(
                  isStarred: isStarred,
                  isUnread: isUnread,
                  onToggleStar: !stars.isReady
                      ? null
                      : () {
                          if (!samePinScope()) return;
                          close();
                          final notifier = ref.read(
                            channelStarsProvider.notifier,
                          );
                          isStarred
                              ? notifier.unstarChannel(channel.id)
                              : notifier.starChannel(channel.id);
                        },
                  onToggleRead: () {
                    close();
                    final timestamp = dateTimeToUnixSeconds(
                      channel.lastMessageAt,
                    );
                    if (isUnread) {
                      onMarkRead?.call();
                      if (timestamp != null) {
                        ref
                            .read(readStateProvider.notifier)
                            .markContextRead(
                              channel.id,
                              timestamp,
                              clearForcedMessages: true,
                            );
                        ref
                            .read(channelsProvider.notifier)
                            .clearObservedUnreadCoveredByRead(
                              channel.id,
                              timestamp,
                            );
                      }
                    } else {
                      ref
                          .read(readStateProvider.notifier)
                          .markContextUnread(channel.id, channelId: channel.id);
                    }
                  },
                ),
                const SizedBox(height: Grid.xs),
              ],
              if (channel.isForum)
                ListTile(
                  contentPadding: EdgeInsets.zero,
                  leading: const Icon(LucideIcons.folderInput),
                  title: Text(context.l10n.channelMoveToSection),
                  onTap: () async {
                    final pageContext = Navigator.of(
                      context,
                      rootNavigator: true,
                    ).context;
                    close();
                    await _showMoveSectionSheet(
                      pageContext,
                      ref,
                      channel: channel,
                      sectionId: sectionId,
                    );
                  },
                ),
              ListTile(
                contentPadding: EdgeInsets.zero,
                leading: Icon(isMuted ? LucideIcons.bell : LucideIcons.bellOff),
                title: Text(
                  isMuted
                      ? (channel.isPersonChat
                            ? context.l10n.channelUnmuteChat
                            : context.l10n.channelUnmuteGroup)
                      : (channel.isPersonChat
                            ? context.l10n.channelMuteChat
                            : context.l10n.channelMuteGroup),
                ),
                onTap: () {
                  close();
                  final notifier = ref.read(channelMutesProvider.notifier);
                  isMuted
                      ? notifier.unmuteChannel(channel.id)
                      : notifier.muteChannel(channel.id);
                },
              ),
              if (dmOther != null)
                ListTile(
                  key: const ValueKey('dm-add-people'),
                  contentPadding: EdgeInsets.zero,
                  leading: const Icon(LucideIcons.userPlus),
                  title: Text(context.l10n.channelAddPeople),
                  onTap: () {
                    final pageContext = Navigator.of(
                      context,
                      rootNavigator: true,
                    ).context;
                    final actions = ref.read(channelActionsProvider);
                    close();
                    showAddPeopleToDm(
                      context: pageContext,
                      actions: actions,
                      otherPubkey: dmOther,
                      dm: currentChannel,
                    ).ignore();
                  },
                ),
              if (channel.isGroupChat)
                ListTile(
                  key: const ValueKey('chat-info-open'),
                  contentPadding: EdgeInsets.zero,
                  leading: const Icon(LucideIcons.info),
                  title: Text(context.l10n.channelChatInfo),
                  onTap: () async {
                    final left = await showChatInfoSheet(
                      context: context,
                      channel: currentChannel,
                    );
                    if (left == true && context.mounted) {
                      Navigator.of(context).pop(true);
                    }
                  },
                ),
              if (channel.isPersonChat)
                RoomGoalsSwitch(
                  channel: currentChannel,
                  canEdit: isDmParticipant && !currentChannel.isArchived,
                  contentPadding: EdgeInsets.zero,
                  onSaved: (enabled) => displayedChannel.value =
                      displayedChannel.value.copyWith(goalsEnabled: enabled),
                ),
              if (!channel.isPersonChat)
                ListTile(
                  contentPadding: EdgeInsets.zero,
                  leading: const Icon(LucideIcons.settings),
                  title: Text(context.l10n.channelManageGroup),
                  onTap: () async {
                    final shouldClose = await showKuzzModalBottomSheet<bool>(
                      context: context,
                      title: context.l10n.channelManageGroup,
                      isScrollControlled: true,
                      showDragHandle: true,
                      constraints: BoxConstraints(
                        maxWidth: 640,
                        maxHeight: MediaQuery.sizeOf(context).height * 0.9,
                      ),
                      builder: (_) => ManageChannelSheet(
                        channel: currentChannel,
                        canEditDetails:
                            canManageLifecycle && !currentChannel.isArchived,
                        onChannelUpdated: (updated) =>
                            displayedChannel.value = updated,
                      ),
                    );
                    if (shouldClose == true && context.mounted) {
                      Navigator.of(context).pop(true);
                    }
                  },
                ),
              if (!channel.isGroupChat)
                ListTile(
                  contentPadding: EdgeInsets.zero,
                  leading: const Icon(LucideIcons.copy),
                  title: Text(
                    channel.isDm
                        ? context.l10n.channelCopyChatName
                        : context.l10n.channelCopyGroupName,
                  ),
                  onTap: () {
                    close();
                    copyToClipboard(
                      context,
                      currentChannel.name,
                      message: channel.isDm
                          ? context.l10n.channelChatNameCopied
                          : context.l10n.channelGroupNameCopied,
                    );
                  },
                ),
              ListTile(
                contentPadding: EdgeInsets.zero,
                leading: const Icon(LucideIcons.fingerprint),
                title: Text(
                  channel.isPersonChat
                      ? context.l10n.channelCopyChatId
                      : context.l10n.channelCopyGroupId,
                ),
                onTap: () {
                  close();
                  copyToClipboard(
                    context,
                    channel.id,
                    message: channel.isPersonChat
                        ? context.l10n.channelChatIdCopied
                        : context.l10n.channelGroupIdCopied,
                  );
                },
              ),
              if (channel.isGroupChat && channel.isMember) ...[
                const SheetDivider(),
                _ActionTile(
                  icon: LucideIcons.logOut,
                  label: context.l10n.channelLeaveChat,
                  destructive: true,
                  onTap: () => _confirmAndRun(
                    context,
                    ref,
                    title: context.l10n.channelLeaveChatTitle,
                    body: context.l10n.channelLeaveChatBody,
                    confirmLabel: context.l10n.channelLeave,
                    errorMessage: context.l10n.channelLeaveChatError,
                    action: () => ref
                        .read(channelActionsProvider)
                        .leaveChannel(channel.id),
                  ),
                ),
              ],
              if (!channel.isPersonChat) ...[
                const SheetDivider(),
                if (channel.isMember && !channel.isArchived)
                  _ActionTile(
                    icon: LucideIcons.logOut,
                    label: context.l10n.channelLeaveGroup,
                    destructive: true,
                    onTap: () => _confirmAndRun(
                      context,
                      ref,
                      title: context.l10n.channelLeaveGroupTitle(
                        currentChannel.name,
                      ),
                      body: context.l10n.channelLeaveGroupBody,
                      confirmLabel: context.l10n.channelLeave,
                      errorMessage: context.l10n.channelLeaveGroupError,
                      action: () => ref
                          .read(channelActionsProvider)
                          .leaveChannel(channel.id),
                    ),
                  ),
                if (lifecycleCapabilitiesLoading)
                  ListTile(
                    enabled: false,
                    leading: KuzzLoadingIndicator(
                      size: 20,
                      semanticLabel:
                          context.l10n.channelGroupActionsLoadingLabel,
                    ),
                    title: Text(context.l10n.channelGroupActionsLoading),
                  )
                else if (lifecycleCapabilitiesUnavailable)
                  ListTile(
                    enabled: false,
                    leading: const Icon(LucideIcons.triangleAlert),
                    title: Text(context.l10n.channelGroupActionsUnavailable),
                  )
                else ...[
                  if (canArchive)
                    _ActionTile(
                      icon: LucideIcons.archive,
                      label: context.l10n.channelArchiveGroup,
                      onTap: () => _confirmAndRun(
                        context,
                        ref,
                        title: context.l10n.channelArchiveGroupTitle(
                          currentChannel.name,
                        ),
                        body: context.l10n.channelArchiveGroupBody,
                        confirmLabel: context.l10n.channelArchive,
                        errorMessage: context.l10n.channelArchiveGroupError,
                        action: () => ref
                            .read(channelActionsProvider)
                            .archiveChannel(channel.id),
                      ),
                    ),
                  if (canUnarchive)
                    _ActionTile(
                      icon: LucideIcons.archiveRestore,
                      label: context.l10n.channelUnarchiveGroup,
                      onTap: () => _confirmAndRun(
                        context,
                        ref,
                        title: context.l10n.channelUnarchiveGroupTitle(
                          currentChannel.name,
                        ),
                        body: context.l10n.channelUnarchiveGroupBody,
                        confirmLabel: context.l10n.channelUnarchive,
                        errorMessage: context.l10n.channelUnarchiveGroupError,
                        action: () => ref
                            .read(channelActionsProvider)
                            .unarchiveChannel(channel.id),
                      ),
                    ),
                  if (canDelete)
                    _ActionTile(
                      icon: LucideIcons.trash2,
                      label: context.l10n.channelDeleteGroup,
                      destructive: true,
                      onTap: () => _confirmAndRun(
                        context,
                        ref,
                        title: context.l10n.channelDeleteGroupTitle(
                          currentChannel.name,
                        ),
                        body: context.l10n.channelDeleteGroupBody,
                        confirmLabel: context.l10n.commonDelete,
                        errorMessage: context.l10n.channelDeleteGroupError,
                        action: () => ref
                            .read(channelActionsProvider)
                            .deleteChannel(channel.id),
                      ),
                    ),
                ],
              ],
            ],
          ),
        ),
      ),
    );
  }
}

class _ChannelQuickActionsRow extends StatelessWidget {
  const _ChannelQuickActionsRow({
    required this.isStarred,
    required this.isUnread,
    required this.onToggleStar,
    required this.onToggleRead,
  });

  final bool isStarred;
  final bool isUnread;
  final VoidCallback? onToggleStar;
  final VoidCallback onToggleRead;

  @override
  Widget build(BuildContext context) => Row(
    children: [
      Expanded(
        child: _ChannelQuickAction(
          icon: isStarred ? LucideIcons.pinOff : LucideIcons.pin,
          label: isStarred
              ? context.l10n.channelUnpin
              : context.l10n.channelPin,
          onTap: onToggleStar,
        ),
      ),
      const SizedBox(width: Grid.twelve),
      Expanded(
        child: _ChannelQuickAction(
          icon: isUnread ? LucideIcons.checkCheck : LucideIcons.circleDot,
          label: isUnread
              ? context.l10n.channelMarkRead
              : context.l10n.channelMarkUnread,
          onTap: onToggleRead,
        ),
      ),
    ],
  );
}

class _ChannelQuickAction extends StatelessWidget {
  const _ChannelQuickAction({
    required this.icon,
    required this.label,
    required this.onTap,
  });

  final IconData icon;
  final String label;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) => InkWell(
    onTap: onTap == null
        ? null
        : () {
            unawaited(HapticFeedback.lightImpact());
            onTap?.call();
          },
    child: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Container(
          height: 68 + (Grid.xxs * 2),
          decoration: BoxDecoration(
            color: context.colors.surfaceContainerHighest,
            borderRadius: BorderRadius.circular(Radii.dialog),
          ),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Icon(icon, size: 22, color: context.colors.onSurface),
              const SizedBox(height: Grid.xxs),
              Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: context.textTheme.labelMedium?.copyWith(
                  color: context.colors.onSurface,
                ),
              ),
            ],
          ),
        ),
      ],
    ),
  );
}

class _ActionTile extends StatelessWidget {
  const _ActionTile({
    required this.icon,
    required this.label,
    required this.onTap,
    this.destructive = false,
  });

  final IconData icon;
  final String label;
  final VoidCallback onTap;
  final bool destructive;

  @override
  Widget build(BuildContext context) => ListTile(
    contentPadding: EdgeInsets.zero,
    leading: Icon(icon, color: destructive ? context.colors.error : null),
    title: Text(
      label,
      style: destructive ? TextStyle(color: context.colors.error) : null,
    ),
    onTap: () {
      unawaited(HapticFeedback.lightImpact());
      onTap();
    },
  );
}

Future<void> _confirmAndRun(
  BuildContext sheetContext,
  WidgetRef ref, {
  required String title,
  required String body,
  required String confirmLabel,
  required String errorMessage,
  required Future<void> Function() action,
}) async {
  final pageContext = Navigator.of(sheetContext, rootNavigator: true).context;
  final confirmed = await showKuzzDialog<bool>(
    context: pageContext,
    builder: (dialogContext) => AlertDialog(
      title: Text(title),
      content: Text(body),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(dialogContext).pop(false),
          child: Text(dialogContext.l10n.commonCancel),
        ),
        FilledButton(
          onPressed: () => Navigator.of(dialogContext).pop(true),
          child: Text(confirmLabel),
        ),
      ],
    ),
  );
  if (confirmed != true) return;
  try {
    await action();
    if (pageContext.mounted) Navigator.of(pageContext).pop(true);
  } catch (error) {
    if (!pageContext.mounted) return;
    ScaffoldMessenger.of(
      pageContext,
    ).showSnackBar(SnackBar(content: Text(errorMessage)));
  }
}

Future<void> _showMoveSectionSheet(
  BuildContext context,
  WidgetRef ref, {
  required Channel channel,
  required String? sectionId,
}) async {
  final sections = [...ref.read(channelSectionsProvider).store.sections]
    ..sort((a, b) => a.order.compareTo(b.order));
  await showKuzzModalBottomSheet<void>(
    context: context,
    showDragHandle: true,
    builder: (sheetContext) => SafeArea(
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
            children: [
              for (final section in sections)
                ListTile(
                  leading: const Icon(LucideIcons.folder),
                  title: Text(section.name),
                  trailing: sectionId == section.id
                      ? Icon(
                          LucideIcons.check,
                          color: sheetContext.colors.primary,
                        )
                      : null,
                  onTap: () {
                    Navigator.of(sheetContext).pop();
                    ref
                        .read(channelSectionsProvider.notifier)
                        .assignChannel(channel.id, section.id);
                  },
                ),
              ListTile(
                leading: const Icon(LucideIcons.folderPlus),
                title: Text(sheetContext.l10n.channelNewSection),
                onTap: () async {
                  Navigator.of(sheetContext).pop();
                  final name = await _showSectionNameDialog(context);
                  if (name == null || name.isEmpty) return;
                  final notifier = ref.read(channelSectionsProvider.notifier);
                  notifier.createSection(name);
                  final created = ref
                      .read(channelSectionsProvider)
                      .store
                      .sections
                      .where((section) => section.name == name.trim())
                      .lastOrNull;
                  if (created != null) {
                    notifier.assignChannel(channel.id, created.id);
                  }
                },
              ),
              if (sectionId != null)
                ListTile(
                  leading: const Icon(LucideIcons.folderMinus),
                  title: Text(sheetContext.l10n.channelRemoveFromSection),
                  onTap: () {
                    Navigator.of(sheetContext).pop();
                    ref
                        .read(channelSectionsProvider.notifier)
                        .unassignChannel(channel.id);
                  },
                ),
            ],
          ),
        ),
      ),
    ),
  );
}

Future<String?> _showSectionNameDialog(BuildContext context) async {
  final controller = TextEditingController();
  final result = await showKuzzDialog<String>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      title: Text(dialogContext.l10n.channelNewSectionTitle),
      content: TextField(controller: controller, autofocus: true),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(dialogContext).pop(),
          child: Text(dialogContext.l10n.commonCancel),
        ),
        FilledButton(
          onPressed: () =>
              Navigator.of(dialogContext).pop(controller.text.trim()),
          child: Text(dialogContext.l10n.channelNewSectionCreate),
        ),
      ],
    ),
  );
  controller.dispose();
  return result;
}
