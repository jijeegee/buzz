import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/auth/auth.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/app_list.dart';
import '../../shared/widgets/app_list_card.dart';
import '../../shared/widgets/buzz_loading_indicator.dart';
import '../../shared/widgets/modal_presentation.dart';

/// Signed-in devices and account security for a token relay: remote
/// sign-out, sign out everywhere else, bot token revocation and account
/// deletion.
class DevicesPage extends HookConsumerWidget {
  const DevicesPage({super.key, required this.origin});

  /// Canonical origin of the token relay.
  final String origin;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final devices = ref.watch(accountDevicesProvider(origin));
    final busy = useState(false);
    final error = useState<String?>(null);
    // Set once the relay has deleted the account: from then on only the
    // local sign-out remains and is retried on its own.
    final accountDeleted = useState(false);
    final cleanupError = useState<String?>(null);

    /// Run [action] after a confirmation; refresh the list when it changed
    /// devices. Failures are shown and leave the list as it was.
    Future<bool> run(
      Future<void> Function(AccountApi api) action, {
      String? done,
      bool refresh = true,
    }) async {
      error.value = null;
      busy.value = true;
      try {
        await action(ref.read(accountApiProvider(origin)));
      } catch (failure) {
        if (context.mounted) error.value = '$failure';
        return false;
      } finally {
        if (context.mounted) busy.value = false;
      }
      if (!context.mounted) return true;
      if (refresh) ref.invalidate(accountDevicesProvider(origin));
      if (done != null) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text(done)));
      }
      return true;
    }

    Future<void> signOutDevice(AccountDevice device) async {
      final ok = await _confirm(
        context,
        title: 'Sign out ${device.name}?',
        body: 'That device will need to sign in again.',
        action: 'Sign out',
      );
      if (!ok) return;
      await run((api) => api.revokeDevice(device.id));
    }

    Future<void> signOutOthers() async {
      final ok = await _confirm(
        context,
        title: 'Sign out all other devices?',
        body:
            'Every device except this one will need to sign in again. '
            'Bots keep working.',
        action: 'Sign out others',
      );
      if (!ok) return;
      await run((api) => api.revokeOtherSessions());
    }

    Future<void> revokeBots() async {
      final ok = await _confirm(
        context,
        title: 'Revoke all bot tokens?',
        body: 'Every bot you own stops working until it gets a new token.',
        action: 'Revoke',
      );
      if (!ok) return;
      await run(
        (api) => api.revokeAllBotTokens(),
        done: 'All bot tokens revoked',
        refresh: false,
      );
    }

    /// The relay already deleted the account and ended this session:
    /// forget it on this device only. A failure here is not a failed
    /// deletion, so it is shown on its own with a local-only retry.
    Future<void> finishLocalCleanup() async {
      cleanupError.value = null;
      busy.value = true;
      try {
        await ref.read(authProvider.notifier).signOut(deviceOnly: true);
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
      final ok = await _confirm(
        context,
        title: 'Delete your account?',
        body:
            'Your account is disabled now and permanently deleted after 30 '
            'days. Signing in again before then restores it. Every device, '
            'including this one, is signed out.',
        action: 'Delete account',
      );
      if (!ok) return;
      final deleted = await run((api) => api.deleteAccount(), refresh: false);
      if (!deleted || !context.mounted) return;
      accountDeleted.value = true;
      await finishLocalCleanup();
    }

    final list = devices.value;
    return Scaffold(
      appBar: AppBar(title: const Text('Devices & security')),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.symmetric(vertical: Grid.xs),
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
                        'Your account was deleted, but this device couldn’t '
                        'finish signing out: ${cleanupError.value}',
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
                      child: const Text('Finish signing out'),
                    ),
                  ],
                ),
              ),
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
            if (list != null)
              AppListCard(
                label: 'Signed-in devices',
                children: [
                  for (final device in list)
                    _DeviceRow(
                      device: device,
                      onSignOut: busy.value || device.current
                          ? null
                          : () => unawaited(signOutDevice(device)),
                    ),
                ],
              )
            else if (devices.hasError)
              Padding(
                padding: const EdgeInsets.all(Grid.sm),
                child: Column(
                  children: [
                    Semantics(
                      liveRegion: true,
                      child: Text(
                        'Couldn’t load devices: ${devices.error}',
                        textAlign: TextAlign.center,
                      ),
                    ),
                    const SizedBox(height: Grid.xs),
                    FilledButton(
                      key: const Key('devices-load-retry'),
                      onPressed: () =>
                          ref.invalidate(accountDevicesProvider(origin)),
                      child: const Text('Try again'),
                    ),
                  ],
                ),
              )
            else
              const Padding(
                padding: EdgeInsets.all(Grid.sm),
                child: Center(
                  child: BuzzLoadingIndicator(
                    size: 32,
                    semanticLabel: 'Loading devices',
                  ),
                ),
              ),
            AppListCard(
              label: 'Security',
              children: [
                AppListRow(
                  key: const Key('devices-sign-out-others'),
                  icon: LucideIcons.monitorSmartphone,
                  title: 'Sign out all other devices',
                  onTap: busy.value ? null : () => unawaited(signOutOthers()),
                ),
                AppListRow(
                  key: const Key('devices-revoke-bots'),
                  icon: LucideIcons.bot,
                  title: 'Revoke all bot tokens',
                  onTap: busy.value ? null : () => unawaited(revokeBots()),
                ),
              ],
            ),
            if (!accountDeleted.value)
              AppListCard(
                children: [
                  AppListRow(
                    key: const Key('devices-delete-account'),
                    icon: LucideIcons.trash2,
                    title: 'Delete account',
                    titleColor: context.colors.error,
                    onTap: busy.value ? null : () => unawaited(deleteAccount()),
                  ),
                ],
              ),
          ],
        ),
      ),
    );
  }
}

class _DeviceRow extends StatelessWidget {
  const _DeviceRow({required this.device, required this.onSignOut});

  final AccountDevice device;
  final VoidCallback? onSignOut;

  @override
  Widget build(BuildContext context) {
    final details = [
      if (device.current) 'This device',
      if (device.platform.isNotEmpty) _platformLabel(device.platform),
      if (!device.current && device.lastSeenAt != null)
        'Last active ${_date(device.lastSeenAt!.toLocal())}',
    ].join(' · ');
    return AppListRow(
      icon: switch (device.platform) {
        'mobile' => LucideIcons.smartphone,
        'web' => LucideIcons.globe,
        'cli' => LucideIcons.terminal,
        _ => LucideIcons.monitor,
      },
      title: device.name,
      subtitle: details.isEmpty ? null : details,
      trailing: device.current
          ? null
          : TextButton(
              key: Key('device-sign-out-${device.id}'),
              onPressed: onSignOut,
              child: Semantics(
                label: 'Sign out ${device.name}',
                excludeSemantics: true,
                child: const Text('Sign out'),
              ),
            ),
    );
  }

  static String _platformLabel(String platform) => switch (platform) {
    'mobile' => 'Mobile',
    'desktop' => 'Desktop',
    'web' => 'Web',
    'cli' => 'CLI',
    _ => platform,
  };

  static String _date(DateTime at) =>
      '${at.year}-${at.month.toString().padLeft(2, '0')}-'
      '${at.day.toString().padLeft(2, '0')}';
}

Future<bool> _confirm(
  BuildContext context, {
  required String title,
  required String body,
  required String action,
}) async {
  final confirmed = await showBuzzDialog<bool>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      title: Text(title),
      content: Text(body),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(dialogContext).pop(false),
          child: const Text('Cancel'),
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
