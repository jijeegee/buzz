import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/auth/auth.dart';
import '../../shared/devices/agent_host_devices.dart';
import '../../shared/devices/device_robot.dart';
import '../../shared/devices/device_robot_icon.dart';
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

    Future<void> renameDevice(AccountDevice device) async {
      final name = await _askDeviceName(context, device.name);
      if (name == null || name == device.name) return;
      await run((api) => api.renameDevice(device.id, name));
    }

    Future<void> chooseRobot(AccountDevice device) async {
      final current = resolveDeviceRobot(
        device.id,
        override: ref.read(
          deviceRobotOverridesProvider,
        )[device.id.trim().toLowerCase()],
      );
      final chosen = await _askDeviceRobot(context, device.name, current);
      if (chosen == null ||
          (chosen.colorIndex == current?.colorIndex &&
              chosen.shapeIndex == current?.shapeIndex)) {
        return;
      }
      error.value = null;
      busy.value = true;
      try {
        await ref
            .read(ownDeviceEventsProvider.notifier)
            .publishDeviceRobot(device.id, chosen);
      } catch (failure) {
        if (context.mounted) error.value = 'Couldn’t save the robot: $failure';
      } finally {
        if (context.mounted) busy.value = false;
      }
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
                      onRename: busy.value
                          ? null
                          : () => unawaited(renameDevice(device)),
                      onChooseRobot: busy.value
                          ? null
                          : () => unawaited(chooseRobot(device)),
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
                  ExpansionTile(
                    key: const Key('devices-account-deletion'),
                    title: const Text('Account deletion'),
                    children: [
                      AppListRow(
                        key: const Key('devices-delete-account'),
                        icon: LucideIcons.trash2,
                        title: 'Delete account',
                        titleColor: context.colors.error,
                        onTap: busy.value
                            ? null
                            : () => unawaited(deleteAccount()),
                      ),
                    ],
                  ),
                ],
              ),
          ],
        ),
      ),
    );
  }
}

class _DeviceRow extends ConsumerWidget {
  const _DeviceRow({
    required this.device,
    required this.onRename,
    required this.onChooseRobot,
    required this.onSignOut,
  });

  final AccountDevice device;
  final VoidCallback? onRename;
  final VoidCallback? onChooseRobot;
  final VoidCallback? onSignOut;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final robotOverride = ref.watch(
      deviceRobotOverridesProvider.select(
        (robots) => robots[device.id.trim().toLowerCase()],
      ),
    );
    final details = [
      if (device.current) 'This device',
      if (device.platform.isNotEmpty) _platformLabel(device.platform),
      if (!device.current && device.lastSeenAt != null)
        'Last active ${_date(device.lastSeenAt!.toLocal())}',
    ].join(' · ');
    // The device's robot: agents running on this device show the same one
    // to their owner, so the owner can match an agent to its computer.
    // Tapping it picks another shape and colour.
    return AppListRowRaw(
      key: Key('device-row-${device.id}'),
      onTap: onRename,
      leading: IconButton(
        key: Key('device-robot-${device.id}'),
        onPressed: onChooseRobot,
        tooltip: 'Change robot for ${device.name}',
        icon: DeviceRobotIcon(
          variant: resolveDeviceRobot(device.id, override: robotOverride),
          size: 24,
          fallbackColor: context.colors.onSurfaceVariant,
        ),
      ),
      title: Text(device.name, style: context.textTheme.bodyLarge),
      subtitle: details.isEmpty
          ? null
          : Text(
              details,
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          IconButton(
            key: Key('device-rename-${device.id}'),
            onPressed: onRename,
            tooltip: 'Rename ${device.name}',
            icon: const Icon(LucideIcons.pencil, size: 18),
          ),
          if (!device.current)
            TextButton(
              key: Key('device-sign-out-${device.id}'),
              onPressed: onSignOut,
              child: Semantics(
                label: 'Sign out ${device.name}',
                excludeSemantics: true,
                child: const Text('Sign out'),
              ),
            ),
        ],
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

/// The robot the user picks for [deviceName]; `null` when cancelled.
Future<DeviceRobotVariant?> _askDeviceRobot(
  BuildContext context,
  String deviceName,
  DeviceRobotVariant? current,
) => showBuzzDialog<DeviceRobotVariant>(
  context: context,
  builder: (_) => DeviceRobotPickerDialog(
    deviceName: deviceName,
    initial:
        current ?? deviceRobotVariantFromIndices(colorIndex: 0, shapeIndex: 0)!,
  ),
);

/// Picks a device robot: one of [deviceRobotShapes] in one of
/// [deviceRobotColors], each option drawn as the robot it gives.
@visibleForTesting
class DeviceRobotPickerDialog extends StatefulWidget {
  const DeviceRobotPickerDialog({
    super.key,
    required this.deviceName,
    required this.initial,
  });

  final String deviceName;
  final DeviceRobotVariant initial;

  @override
  State<DeviceRobotPickerDialog> createState() =>
      _DeviceRobotPickerDialogState();
}

class _DeviceRobotPickerDialogState extends State<DeviceRobotPickerDialog> {
  late int _color = widget.initial.colorIndex;
  late int _shape = widget.initial.shapeIndex;

  DeviceRobotVariant _robot(int color, int shape) =>
      deviceRobotVariantFromIndices(colorIndex: color, shapeIndex: shape)!;

  Widget _option({
    required Key key,
    required String label,
    required bool selected,
    required DeviceRobotVariant robot,
    required VoidCallback onTap,
  }) {
    return Semantics(
      label: label,
      selected: selected,
      button: true,
      excludeSemantics: true,
      child: InkWell(
        key: key,
        onTap: onTap,
        customBorder: const CircleBorder(),
        child: Container(
          width: 44,
          height: 44,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            border: Border.all(
              color: selected ? context.colors.primary : Colors.transparent,
              width: 2,
            ),
          ),
          child: DeviceRobotIcon(variant: robot, size: 28),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final sectionStyle = context.textTheme.labelMedium?.copyWith(
      color: context.colors.onSurfaceVariant,
    );
    return AlertDialog(
      title: Text('Robot for ${widget.deviceName}'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Center(
              child: DeviceRobotIcon(
                key: const Key('device-robot-preview'),
                variant: _robot(_color, _shape),
                size: 56,
                semanticLabel: 'Selected robot',
              ),
            ),
            const SizedBox(height: Grid.xs),
            Text('Shape', style: sectionStyle),
            const SizedBox(height: Grid.half),
            Wrap(
              spacing: Grid.half,
              runSpacing: Grid.half,
              children: [
                for (var shape = 0; shape < deviceRobotShapes.length; shape++)
                  _option(
                    key: Key('device-robot-shape-${deviceRobotShapes[shape]}'),
                    label: 'Shape ${deviceRobotShapes[shape]}',
                    selected: shape == _shape,
                    robot: _robot(_color, shape),
                    onTap: () => setState(() => _shape = shape),
                  ),
              ],
            ),
            const SizedBox(height: Grid.xs),
            Text('Colour', style: sectionStyle),
            const SizedBox(height: Grid.half),
            Wrap(
              spacing: Grid.half,
              runSpacing: Grid.half,
              children: [
                for (var color = 0; color < deviceRobotColors.length; color++)
                  _option(
                    key: Key('device-robot-color-$color'),
                    label: 'Colour ${color + 1}',
                    selected: color == _color,
                    robot: _robot(color, _shape),
                    onTap: () => setState(() => _color = color),
                  ),
              ],
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          key: const Key('device-robot-save'),
          onPressed: () => Navigator.of(context).pop(_robot(_color, _shape)),
          child: const Text('Save'),
        ),
      ],
    );
  }
}

/// The new name for a device, trimmed; `null` when cancelled or empty.
Future<String?> _askDeviceName(BuildContext context, String current) async {
  final name = await showBuzzDialog<String>(
    context: context,
    builder: (_) => _DeviceNameDialog(initial: current),
  );
  final trimmed = name?.trim() ?? '';
  return trimmed.isEmpty ? null : trimmed;
}

class _DeviceNameDialog extends StatefulWidget {
  const _DeviceNameDialog({required this.initial});

  final String initial;

  @override
  State<_DeviceNameDialog> createState() => _DeviceNameDialogState();
}

class _DeviceNameDialogState extends State<_DeviceNameDialog> {
  late final _controller = TextEditingController(text: widget.initial);

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _submit() => Navigator.of(context).pop(_controller.text);

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Rename device'),
      content: TextField(
        key: const Key('device-rename-field'),
        controller: _controller,
        autofocus: true,
        maxLength: 64,
        textInputAction: TextInputAction.done,
        decoration: const InputDecoration(labelText: 'Device name'),
        onSubmitted: (_) => _submit(),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          key: const Key('device-rename-save'),
          onPressed: _submit,
          child: const Text('Save'),
        ),
      ],
    );
  }
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
