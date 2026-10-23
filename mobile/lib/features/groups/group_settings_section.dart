import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:share_plus/share_plus.dart';

import '../../shared/clipboard_utils.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/relay/relay.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/app_list.dart';
import '../../shared/widgets/app_list_card.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/widgets/modal_presentation.dart';
import '../channels/channel.dart';
import '../channels/channel_management_provider.dart';
import 'group_choice_sheet.dart';
import 'groups_api.dart';

/// A group's server-side settings: visibility, public address and invite
/// policy (`GET /api/groups/{id}`).
final groupInfoProvider = FutureProvider.autoDispose
    .family<GroupSummary, String>(
      (ref, id) => ref.watch(groupsApiProvider).group(id),
      retry: noGroupsRetry,
    );

/// `<host>/g/@`, the fixed part of a public address.
String groupAddressPrefix(String relayBaseUrl) {
  final uri = Uri.tryParse(relayBaseUrl);
  final host = uri == null || uri.host.isEmpty
      ? relayBaseUrl
      : uri.hasPort
      ? '${uri.host}:${uri.port}'
      : uri.host;
  return '$host/g/@';
}

/// User-facing text for a refused address change.
String publicAddressErrorMessage(Object error) {
  if (error is GroupsApiException) {
    if (error.isConflict) return appL10n.groupsAddressTaken;
    if (error.statusCode == 400) {
      return error.message.contains('reserved')
          ? appL10n.groupsAddressReserved
          : appL10n.groupsAddressRules;
    }
  }
  return appL10n.groupsSaveError;
}

/// Group settings rows: Visibility, Public address, Invite link and Who can
/// invite. [canManage] is owner/admin.
class GroupSettingsCard extends HookConsumerWidget {
  const GroupSettingsCard({
    super.key,
    required this.channel,
    required this.canManage,
    this.onVisibilityChanged,
  });

  final Channel channel;
  final bool canManage;
  final ValueChanged<String>? onVisibilityChanged;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final infoAsync = ref.watch(groupInfoProvider(channel.id));
    final info = infoAsync.value;
    final saving = useState(false);
    final prefix = groupAddressPrefix(ref.watch(relayConfigProvider).baseUrl);
    final isPublic = (info?.visibility ?? channel.visibility) == 'open';
    final policy = info?.invitePolicy ?? 'members';
    final canSeeLink = canManage || policy != 'admins';
    final l10n = context.l10n;
    final chevron = Icon(
      LucideIcons.chevronRightDir,
      size: 18,
      color: context.colors.onSurfaceVariant,
    );

    void say(String message) => ScaffoldMessenger.maybeOf(
      context,
    )?.showSnackBar(SnackBar(content: Text(message)));

    Future<void> changeVisibility() async {
      final next = await showGroupChoiceSheet<String>(
        context: context,
        title: l10n.groupsVisibility,
        selected: isPublic ? 'open' : 'private',
        options: [
          GroupChoice(
            value: 'open',
            label: l10n.groupsPublicGroup,
            key: const ValueKey('group-visibility-open'),
          ),
          GroupChoice(
            value: 'private',
            label: l10n.groupsPrivateGroup,
            key: const ValueKey('group-visibility-private'),
          ),
        ],
      );
      if (next == null || next == (isPublic ? 'open' : 'private')) return;
      if (!context.mounted) return;
      final toPublic = next == 'open';
      final confirmed = await showGroupConfirmDialog(
        context,
        title: toPublic
            ? l10n.groupsMakePublicTitle
            : l10n.groupsMakePrivateTitle,
        body: toPublic ? l10n.groupsToPublicBody : l10n.groupsToPrivateBody,
        confirmLabel: toPublic ? l10n.groupsMakePublic : l10n.groupsMakePrivate,
      );
      if (!confirmed) return;
      saving.value = true;
      try {
        await ref
            .read(channelActionsProvider)
            .updateChannel(channelId: channel.id, visibility: next);
        ref.invalidate(groupInfoProvider(channel.id));
        onVisibilityChanged?.call(next);
      } catch (_) {
        say(l10n.groupsSettingChangeError);
      } finally {
        if (context.mounted) saving.value = false;
      }
    }

    Future<void> changeInvitePolicy() async {
      final next = await showGroupChoiceSheet<String>(
        context: context,
        title: l10n.groupsWhoCanInvite,
        selected: policy,
        options: [
          GroupChoice(
            value: 'members',
            label: l10n.groupsAllMembers,
            key: const ValueKey('group-invite-policy-members'),
          ),
          GroupChoice(
            value: 'admins',
            label: l10n.groupsAdminsOnly,
            key: const ValueKey('group-invite-policy-admins'),
          ),
        ],
      );
      if (next == null || next == policy) return;
      try {
        await ref.read(groupsApiProvider).setInvitePolicy(channel.id, next);
        ref.invalidate(groupInfoProvider(channel.id));
      } catch (_) {
        say(l10n.groupsSettingChangeError);
      }
    }

    final rows = <Widget>[
      if (canManage)
        AppListRow(
          key: const ValueKey('group-settings-visibility'),
          icon: isPublic ? LucideIcons.globe : LucideIcons.lock,
          title: l10n.groupsVisibility,
          value: isPublic ? l10n.groupsPublicGroup : l10n.groupsPrivateGroup,
          trailing: saving.value
              ? const KuzzLoadingIndicator(size: 18)
              : chevron,
          onTap: saving.value || info == null ? null : changeVisibility,
        ),
      if (canManage && isPublic)
        AppListRow(
          key: const ValueKey('group-settings-address'),
          icon: LucideIcons.atSign,
          title: l10n.groupsPublicAddress,
          subtitle: info?.handle == null
              ? l10n.groupsNotSet
              : '$prefix${info!.handle}',
          trailing: chevron,
          onTap: info == null
              ? null
              : () => unawaited(
                  showPublicAddressSheet(
                    context,
                    groupId: channel.id,
                    prefix: prefix,
                    current: info.handle,
                  ),
                ),
        ),
      if (info != null && canSeeLink)
        AppListRow(
          key: const ValueKey('group-settings-invite-link'),
          icon: LucideIcons.link,
          title: l10n.groupsInviteLink,
          trailing: chevron,
          onTap: () => unawaited(
            showInviteLinkSheet(
              context,
              groupId: channel.id,
              canReset: canManage,
            ),
          ),
        ),
      if (canManage && !isPublic)
        AppListRow(
          key: const ValueKey('group-settings-invite-policy'),
          icon: LucideIcons.userPlus,
          title: l10n.groupsWhoCanInvite,
          value: policy == 'admins'
              ? l10n.groupsAdminsOnly
              : l10n.groupsAllMembers,
          trailing: chevron,
          onTap: info == null ? null : changeInvitePolicy,
        ),
    ];
    if (rows.isEmpty) return const SizedBox.shrink();
    return AppListCard(
      key: const ValueKey('channel-details-group-settings-card'),
      verticalPadding: Grid.twelve,
      children: rows,
    );
  }
}

/// A confirm dialog; true when [confirmLabel] was tapped.
Future<bool> showGroupConfirmDialog(
  BuildContext context, {
  required String title,
  required String body,
  required String confirmLabel,
}) async {
  final confirmed = await showDialog<bool>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text(title),
      content: Text(body),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(false),
          child: Text(context.l10n.groupsCancel),
        ),
        FilledButton(
          key: const ValueKey('group-confirm'),
          onPressed: () => Navigator.of(context).pop(true),
          child: Text(confirmLabel),
        ),
      ],
    ),
  );
  return confirmed == true;
}

/// Edits the public address: `<host>/g/@` + handle, Save.
Future<void> showPublicAddressSheet(
  BuildContext context, {
  required String groupId,
  required String prefix,
  required String? current,
}) => showKuzzModalBottomSheet<void>(
  context: context,
  title: context.l10n.groupsPublicAddress,
  isScrollControlled: true,
  showDragHandle: true,
  builder: (_) =>
      PublicAddressSheet(groupId: groupId, prefix: prefix, current: current),
);

class PublicAddressSheet extends HookConsumerWidget {
  const PublicAddressSheet({
    super.key,
    required this.groupId,
    required this.prefix,
    required this.current,
  });

  final String groupId;
  final String prefix;
  final String? current;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final controller = useTextEditingController(text: current ?? '');
    useListenable(controller);
    final saving = useState(false);
    final error = useState<String?>(null);
    final handle = controller.text.trim().replaceFirst('@', '').toLowerCase();
    final changed = handle != (current ?? '');

    Future<void> save() async {
      if (saving.value || !changed) return;
      final navigator = Navigator.of(context);
      saving.value = true;
      error.value = null;
      try {
        await ref
            .read(groupsApiProvider)
            .setHandle(groupId, handle.isEmpty ? null : handle);
        ref.invalidate(groupInfoProvider(groupId));
        navigator.pop();
      } catch (e) {
        if (context.mounted) error.value = publicAddressErrorMessage(e);
      } finally {
        if (context.mounted) saving.value = false;
      }
    }

    return SafeArea(
      top: false,
      child: Padding(
        padding: EdgeInsets.fromLTRB(
          Grid.gutter,
          0,
          Grid.gutter,
          MediaQuery.viewInsetsOf(context).bottom + Grid.xs,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            DecoratedBox(
              decoration: BoxDecoration(
                border: Border.all(
                  color: error.value == null
                      ? context.colors.outlineVariant.withValues(alpha: 0.8)
                      : context.colors.error,
                ),
                borderRadius: BorderRadius.circular(Radii.card),
              ),
              child: Row(
                children: [
                  Padding(
                    padding: const EdgeInsetsDirectional.only(
                      start: Grid.twelve,
                    ),
                    child: Text(
                      prefix,
                      style: context.textTheme.bodyLarge?.copyWith(
                        color: context.colors.onSurfaceVariant,
                      ),
                    ),
                  ),
                  Expanded(
                    child: TextField(
                      key: const ValueKey('public-address-input'),
                      controller: controller,
                      autofocus: true,
                      autocorrect: false,
                      enableSuggestions: false,
                      enabled: !saving.value,
                      onChanged: (_) => error.value = null,
                      onSubmitted: (_) => unawaited(save()),
                      style: context.textTheme.bodyLarge,
                      decoration: InputDecoration(
                        hintText: context.l10n.groupsAddressHint,
                        border: InputBorder.none,
                        enabledBorder: InputBorder.none,
                        focusedBorder: InputBorder.none,
                        isDense: true,
                        contentPadding: EdgeInsets.symmetric(
                          vertical: Grid.twelve,
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
            if (error.value case final message?) ...[
              const SizedBox(height: Grid.half),
              Text(
                message,
                key: const ValueKey('public-address-error'),
                style: context.textTheme.bodySmall?.copyWith(
                  color: context.colors.error,
                ),
              ),
            ],
            const SizedBox(height: Grid.xs),
            FilledButton(
              key: const ValueKey('public-address-save'),
              onPressed: changed && !saving.value
                  ? () => unawaited(save())
                  : null,
              child: Text(
                saving.value
                    ? context.l10n.groupsSaving
                    : context.l10n.groupsSave,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// The group's invite link with Copy link, Share link and (owner/admin)
/// Reset link.
Future<void> showInviteLinkSheet(
  BuildContext context, {
  required String groupId,
  required bool canReset,
}) => showKuzzModalBottomSheet<void>(
  context: context,
  title: context.l10n.groupsInviteLink,
  isScrollControlled: true,
  showDragHandle: true,
  builder: (_) => InviteLinkSheet(groupId: groupId, canReset: canReset),
);

class InviteLinkSheet extends HookConsumerWidget {
  const InviteLinkSheet({
    super.key,
    required this.groupId,
    required this.canReset,
  });

  final String groupId;
  final bool canReset;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final link = useState<GroupInviteLink?>(null);
    final failed = useState(false);
    final resetting = useState(false);
    final loadSerial = useState(0);

    useEffect(() {
      var cancelled = false;
      failed.value = false;
      ref
          .read(groupsApiProvider)
          .inviteLink(groupId)
          .then(
            (value) {
              if (!cancelled) link.value = value;
            },
            onError: (Object _) {
              if (!cancelled) failed.value = true;
            },
          );
      return () => cancelled = true;
    }, [loadSerial.value]);

    Future<void> reset() async {
      final confirmed = await showGroupConfirmDialog(
        context,
        title: context.l10n.groupsResetLinkTitle,
        body: context.l10n.groupsResetLinkBody,
        confirmLabel: context.l10n.groupsResetLink,
      );
      if (!confirmed || !context.mounted) return;
      resetting.value = true;
      try {
        link.value = await ref.read(groupsApiProvider).resetInviteLink(groupId);
      } catch (_) {
        if (context.mounted) {
          ScaffoldMessenger.maybeOf(context)?.showSnackBar(
            SnackBar(content: Text(context.l10n.groupsResetLinkError)),
          );
        }
      } finally {
        if (context.mounted) resetting.value = false;
      }
    }

    final current = link.value;
    final Widget content;
    if (current == null) {
      content = failed.value
          ? Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  context.l10n.groupsInviteLinkLoadError,
                  style: context.textTheme.bodyMedium,
                ),
                TextButton(
                  onPressed: () => loadSerial.value++,
                  child: Text(context.l10n.groupsTryAgain),
                ),
              ],
            )
          : const Padding(
              padding: EdgeInsets.symmetric(vertical: Grid.sm),
              child: Center(child: KuzzLoadingIndicator()),
            );
    } else {
      content = Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(
            padding: const EdgeInsets.all(Grid.twelve),
            decoration: BoxDecoration(
              color: context.colors.surface,
              border: Border.all(color: context.colors.outlineVariant),
              borderRadius: BorderRadius.circular(Radii.card),
            ),
            child: SelectableText(
              current.url,
              key: const ValueKey('invite-link-url'),
              style: context.textTheme.bodyMedium,
            ),
          ),
          const SizedBox(height: Grid.xs),
          Row(
            children: [
              Expanded(
                child: FilledButton.tonalIcon(
                  key: const ValueKey('invite-link-copy'),
                  icon: const Icon(LucideIcons.copy, size: 18),
                  label: Text(context.l10n.groupsCopyLink),
                  onPressed: () => unawaited(
                    copyToClipboard(
                      context,
                      current.url,
                      message: context.l10n.groupsLinkCopied,
                    ),
                  ),
                ),
              ),
              const SizedBox(width: Grid.xxs),
              Expanded(
                child: FilledButton.tonalIcon(
                  key: const ValueKey('invite-link-share'),
                  icon: const Icon(LucideIcons.share2, size: 18),
                  label: Text(context.l10n.groupsShareLink),
                  onPressed: () {
                    final box = context.findRenderObject() as RenderBox?;
                    unawaited(
                      SharePlus.instance.share(
                        ShareParams(
                          text: current.url,
                          sharePositionOrigin: box == null
                              ? null
                              : box.localToGlobal(Offset.zero) & box.size,
                        ),
                      ),
                    );
                  },
                ),
              ),
            ],
          ),
          if (canReset) ...[
            const SizedBox(height: Grid.xxs),
            TextButton(
              key: const ValueKey('invite-link-reset'),
              onPressed: resetting.value ? null : () => unawaited(reset()),
              style: TextButton.styleFrom(
                foregroundColor: context.colors.error,
              ),
              child: Text(
                resetting.value
                    ? context.l10n.groupsResetting
                    : context.l10n.groupsResetLink,
              ),
            ),
          ],
        ],
      );
    }

    return SafeArea(
      top: false,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(
          Grid.gutter,
          0,
          Grid.gutter,
          Grid.xs,
        ),
        child: content,
      ),
    );
  }
}
