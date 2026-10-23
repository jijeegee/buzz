import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/auth/auth.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/app_list.dart';
import '../../shared/widgets/app_list_card.dart';
import '../../shared/widgets/modal_presentation.dart';

/// The word typed in the second step of account deletion.
const accountDeletionConfirmWord = 'delete';

/// Settings › Account: disconnect every agent of the account at once, as on
/// desktop. Agents on a computer reconnect the next time they start.
class DisconnectAgentsRow extends HookConsumerWidget {
  const DisconnectAgentsRow({super.key, required this.origin});

  /// Canonical origin of the token relay.
  final String origin;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final busy = useState(false);

    Future<void> disconnect() async {
      final ok = await confirmDestructive(
        context,
        title: context.l10n.settingsDevicesRevokeBotsTitle,
        body: context.l10n.settingsDevicesRevokeBotsBody,
        action: context.l10n.settingsDevicesRevokeBotsAction,
      );
      if (!ok || !context.mounted) return;
      busy.value = true;
      final l10n = context.l10n;
      String message;
      try {
        await ref.read(accountApiProvider(origin)).revokeAllBotTokens();
        message = l10n.settingsDevicesRevokeBotsDone;
      } catch (failure) {
        message = l10n.settingsDevicesRevokeBotsFailed('$failure');
      } finally {
        if (context.mounted) busy.value = false;
      }
      if (!context.mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(message)));
    }

    return AppListRow(
      key: const Key('devices-revoke-bots'),
      icon: LucideIcons.bot,
      title: context.l10n.settingsDevicesRevokeBots,
      onTap: busy.value ? null : () => unawaited(disconnect()),
    );
  }
}

/// The bottom of Settings: account deletion, folded away like desktop's.
/// Deleting takes two steps — what happens, then typing
/// [accountDeletionConfirmWord].
class AccountDeletionSection extends HookConsumerWidget {
  const AccountDeletionSection({super.key, required this.origin});

  /// Canonical origin of the token relay.
  final String origin;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final busy = useState(false);
    final error = useState<String?>(null);
    // Set once the relay has deleted the account: from then on only the
    // local sign-out remains and is retried on its own.
    final accountDeleted = useState(false);
    final cleanupError = useState<String?>(null);

    /// The relay already deleted the account and ended this session:
    /// forget it on this device only. A failure here is not a failed
    /// deletion, so it is shown on its own with a local-only retry.
    Future<void> finishLocalCleanup() async {
      cleanupError.value = null;
      busy.value = true;
      try {
        await ref
            .read(authProvider.notifier)
            .removeActiveCommunity(deviceOnly: true);
      } catch (failure) {
        if (context.mounted) cleanupError.value = '$failure';
        return;
      } finally {
        if (context.mounted) busy.value = false;
      }
      if (context.mounted) {
        Navigator.of(context).popUntil((route) => route.isFirst);
      }
    }

    Future<void> deleteAccount() async {
      final understood = await confirmDestructive(
        context,
        title: context.l10n.settingsDevicesDeleteAccountTitle,
        body: context.l10n.settingsDevicesDeleteAccountBody,
        action: context.l10n.settingsDevicesDeleteContinue,
      );
      if (!understood || !context.mounted) return;
      final typed = await _confirmByTyping(context);
      if (!typed || !context.mounted) return;
      error.value = null;
      busy.value = true;
      try {
        await ref.read(accountApiProvider(origin)).deleteAccount();
      } catch (failure) {
        if (context.mounted) error.value = '$failure';
        return;
      } finally {
        if (context.mounted) busy.value = false;
      }
      if (!context.mounted) return;
      accountDeleted.value = true;
      await finishLocalCleanup();
    }

    return AppListCard(
      verticalPadding: Grid.twelve,
      children: [
        if (accountDeleted.value && cleanupError.value != null)
          Padding(
            padding: const EdgeInsets.symmetric(
              horizontal: Grid.gutter,
              vertical: Grid.xxs,
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Semantics(
                  liveRegion: true,
                  child: Text(
                    context.l10n.settingsDevicesLocalCleanupFailed(
                      cleanupError.value!,
                    ),
                    key: const Key('devices-local-cleanup-error'),
                    style: context.textTheme.bodySmall?.copyWith(
                      color: context.colors.error,
                    ),
                  ),
                ),
                const SizedBox(height: Grid.xxs),
                FilledButton(
                  key: const Key('devices-local-cleanup-retry'),
                  onPressed: busy.value
                      ? null
                      : () => unawaited(finishLocalCleanup()),
                  child: Text(context.l10n.settingsDevicesFinishSignOut),
                ),
              ],
            ),
          )
        else
          ExpansionTile(
            key: const Key('devices-account-deletion'),
            title: Text(context.l10n.settingsDevicesAccountDeletion),
            children: [
              if (error.value != null)
                Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: Grid.gutter,
                    vertical: Grid.xxs,
                  ),
                  child: Semantics(
                    liveRegion: true,
                    child: Text(
                      error.value!,
                      key: const Key('devices-error'),
                      style: context.textTheme.bodySmall?.copyWith(
                        color: context.colors.error,
                      ),
                    ),
                  ),
                ),
              AppListRow(
                key: const Key('devices-delete-account'),
                icon: LucideIcons.trash2,
                title: context.l10n.settingsDevicesDeleteAccount,
                subtitle: context.l10n.settingsDevicesDeleteAccountHint,
                titleColor: context.colors.error,
                onTap: busy.value ? null : () => unawaited(deleteAccount()),
              ),
            ],
          ),
      ],
    );
  }
}

/// Second step of account deletion: the button unlocks only once
/// [accountDeletionConfirmWord] is typed.
Future<bool> _confirmByTyping(BuildContext context) async {
  final confirmed = await showKuzzDialog<bool>(
    context: context,
    builder: (dialogContext) => const _TypeToDeleteDialog(),
  );
  return confirmed ?? false;
}

class _TypeToDeleteDialog extends HookWidget {
  const _TypeToDeleteDialog();

  @override
  Widget build(BuildContext context) {
    final controller = useTextEditingController();
    useListenable(controller);
    final matches =
        controller.text.trim().toLowerCase() == accountDeletionConfirmWord;
    return AlertDialog(
      title: Text(
        context.l10n.settingsDevicesDeleteTypeTitle(accountDeletionConfirmWord),
      ),
      content: TextField(
        key: const Key('account-delete-confirm-field'),
        controller: controller,
        autocorrect: false,
        enableSuggestions: false,
        autofocus: true,
        decoration: const InputDecoration(hintText: accountDeletionConfirmWord),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(false),
          child: Text(context.l10n.commonCancel),
        ),
        FilledButton(
          key: const Key('account-delete-confirm'),
          onPressed: matches ? () => Navigator.of(context).pop(true) : null,
          style: FilledButton.styleFrom(backgroundColor: context.colors.error),
          child: Text(context.l10n.settingsDevicesDeleteAccount),
        ),
      ],
    );
  }
}

/// A destructive confirmation shared by the account and device settings.
Future<bool> confirmDestructive(
  BuildContext context, {
  required String title,
  required String body,
  required String action,
}) async {
  final confirmed = await showKuzzDialog<bool>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      title: Text(title),
      content: Text(body),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(dialogContext).pop(false),
          child: Text(context.l10n.commonCancel),
        ),
        FilledButton(
          key: const Key('devices-confirm'),
          onPressed: () => Navigator.of(dialogContext).pop(true),
          style: FilledButton.styleFrom(
            backgroundColor: dialogContext.colors.error,
          ),
          child: Text(action),
        ),
      ],
    ),
  );
  return confirmed ?? false;
}
