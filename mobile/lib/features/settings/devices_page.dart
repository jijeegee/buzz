import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/devices/agent_host_devices.dart';
import '../../shared/devices/device_robot.dart';
import '../../shared/devices/device_robot_icon.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/app_list.dart';
import '../../shared/widgets/app_list_card.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/widgets/modal_presentation.dart';
import 'account_actions.dart';

/// Settings › Devices & security for a token relay, as on desktop: the
/// signed-in devices — rename, pick each one's robot, sign one out —
/// signing out every other device, and disconnecting every agent.
class DevicesPage extends HookConsumerWidget {
  const DevicesPage({super.key, required this.origin, this.agents = true});

  /// Canonical origin of the token relay.
  final String origin;

  /// Offer disconnecting every agent. Off for a Google key account, whose
  /// agents the relay cannot cut off yet (desktop leaves it out too).
  final bool agents;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final devices = ref.watch(accountDevicesProvider(origin));
    final busy = useState(false);
    final error = useState<String?>(null);

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
      final ok = await confirmDestructive(
        context,
        title: context.l10n.settingsDevicesSignOutDeviceTitle(device.name),
        body: context.l10n.settingsDevicesSignOutDeviceBody,
        action: context.l10n.settingsDevicesSignOut,
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
        if (context.mounted) {
          error.value = context.l10n.settingsDevicesRobotSaveFailed('$failure');
        }
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    Future<void> signOutOthers() async {
      final ok = await confirmDestructive(
        context,
        title: context.l10n.settingsDevicesSignOutOthersTitle,
        body: context.l10n.settingsDevicesSignOutOthersBody,
        action: context.l10n.settingsDevicesSignOutOthersAction,
      );
      if (!ok) return;
      await run((api) => api.revokeOtherSessions());
    }

    final list = devices.value;
    return Scaffold(
      appBar: AppBar(title: Text(context.l10n.settingsDevicesTitle)),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.symmetric(vertical: Grid.xs),
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
            if (list != null)
              AppListCard(
                label: context.l10n.settingsDevicesSignedIn,
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
                        context.l10n.settingsDevicesLoadFailed(
                          '${devices.error}',
                        ),
                        textAlign: TextAlign.center,
                      ),
                    ),
                    const SizedBox(height: Grid.xs),
                    FilledButton(
                      key: const Key('devices-load-retry'),
                      onPressed: () =>
                          ref.invalidate(accountDevicesProvider(origin)),
                      child: Text(context.l10n.commonTryAgain),
                    ),
                  ],
                ),
              )
            else
              Padding(
                padding: const EdgeInsets.all(Grid.sm),
                child: Center(
                  child: KuzzLoadingIndicator(
                    size: 32,
                    semanticLabel: context.l10n.settingsDevicesLoading,
                  ),
                ),
              ),
            AppListCard(
              children: [
                AppListRow(
                  key: const Key('devices-sign-out-others'),
                  icon: LucideIcons.monitorSmartphone,
                  title: context.l10n.settingsDevicesSignOutOthers,
                  onTap: busy.value ? null : () => unawaited(signOutOthers()),
                ),
                if (agents) DisconnectAgentsRow(origin: origin),
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
      if (device.current) context.l10n.settingsDevicesThisDevice,
      if (device.platform.isNotEmpty)
        _platformLabel(context.l10n, device.platform),
      if (!device.current && device.lastSeenAt != null)
        context.l10n.settingsDevicesLastActive(
          _date(device.lastSeenAt!.toLocal()),
        ),
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
        tooltip: context.l10n.settingsDevicesChangeRobot(device.name),
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
            tooltip: context.l10n.settingsDevicesRenameTooltip(device.name),
            icon: const Icon(LucideIcons.pencil, size: 18),
          ),
          if (!device.current)
            TextButton(
              key: Key('device-sign-out-${device.id}'),
              onPressed: onSignOut,
              child: Semantics(
                label: context.l10n.settingsDevicesSignOutLabel(device.name),
                excludeSemantics: true,
                child: Text(context.l10n.settingsDevicesSignOut),
              ),
            ),
        ],
      ),
    );
  }

  static String _platformLabel(AppLocalizations l10n, String platform) =>
      switch (platform) {
        'mobile' => l10n.settingsDevicesPlatformMobile,
        'desktop' => l10n.settingsDevicesPlatformDesktop,
        'web' => l10n.settingsDevicesPlatformWeb,
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
) => showKuzzDialog<DeviceRobotVariant>(
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
      title: Text(context.l10n.settingsDevicesRobotTitle(widget.deviceName)),
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
                semanticLabel: context.l10n.settingsDevicesRobotSelected,
              ),
            ),
            const SizedBox(height: Grid.xs),
            Text(context.l10n.settingsDevicesRobotShape, style: sectionStyle),
            const SizedBox(height: Grid.half),
            Wrap(
              spacing: Grid.half,
              runSpacing: Grid.half,
              children: [
                for (var shape = 0; shape < deviceRobotShapes.length; shape++)
                  _option(
                    key: Key('device-robot-shape-${deviceRobotShapes[shape]}'),
                    label: context.l10n.settingsDevicesRobotShapeOption(
                      deviceRobotShapes[shape],
                    ),
                    selected: shape == _shape,
                    robot: _robot(_color, shape),
                    onTap: () => setState(() => _shape = shape),
                  ),
              ],
            ),
            const SizedBox(height: Grid.xs),
            Text(context.l10n.settingsDevicesRobotColour, style: sectionStyle),
            const SizedBox(height: Grid.half),
            Wrap(
              spacing: Grid.half,
              runSpacing: Grid.half,
              children: [
                for (var color = 0; color < deviceRobotColors.length; color++)
                  _option(
                    key: Key('device-robot-color-$color'),
                    label: context.l10n.settingsDevicesRobotColourOption(
                      color + 1,
                    ),
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
          child: Text(context.l10n.commonCancel),
        ),
        FilledButton(
          key: const Key('device-robot-save'),
          onPressed: () => Navigator.of(context).pop(_robot(_color, _shape)),
          child: Text(context.l10n.commonSave),
        ),
      ],
    );
  }
}

/// The new name for a device, trimmed; `null` when cancelled or empty.
Future<String?> _askDeviceName(BuildContext context, String current) async {
  final name = await showKuzzDialog<String>(
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
      title: Text(context.l10n.settingsDevicesRenameTitle),
      content: TextField(
        key: const Key('device-rename-field'),
        controller: _controller,
        autofocus: true,
        maxLength: 64,
        textInputAction: TextInputAction.done,
        decoration: InputDecoration(
          labelText: context.l10n.settingsDevicesNameLabel,
        ),
        onSubmitted: (_) => _submit(),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: Text(context.l10n.commonCancel),
        ),
        FilledButton(
          key: const Key('device-rename-save'),
          onPressed: _submit,
          child: Text(context.l10n.commonSave),
        ),
      ],
    );
  }
}
