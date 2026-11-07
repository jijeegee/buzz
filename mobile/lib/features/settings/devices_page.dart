import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/auth/token/session_key.dart';
import '../../shared/devices/agent_host_devices.dart';
import '../../shared/devices/device_robot.dart';
import '../../shared/devices/device_robot_icon.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/mentions/agent_identity_provider.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/app_list.dart';
import '../../shared/widgets/app_list_card.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/widgets/modal_presentation.dart';
import '../agents/agent_settings_provider.dart';
import '../agents/external_agent_access.dart';
import '../agents/external_agent_sheet.dart';
import '../agents/owner_tools_row.dart';
import 'account_actions.dart';

/// A stalled relay must not hold up a disconnect.
const _narrowLimit = Duration(seconds: 10);

/// Settings › Devices & security for a token relay, as on desktop: the
/// signed-in devices — rename, pick each one's robot, sign one out — the
/// external agents under the computer they run on, signing out every other
/// device, and disconnecting every agent.
class DevicesPage extends HookConsumerWidget {
  const DevicesPage({super.key, required this.sessionKey, this.agents = true});

  /// The account of the token relay.
  final SessionKey sessionKey;

  /// Offer disconnecting every agent. Off for a Google key account, whose
  /// agents the relay cannot cut off yet (desktop leaves it out too).
  final bool agents;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final devices = ref.watch(accountDevicesProvider(sessionKey));
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
        await action(ref.read(accountApiProvider(sessionKey)));
      } catch (failure) {
        if (context.mounted) error.value = '$failure';
        return false;
      } finally {
        if (context.mounted) busy.value = false;
      }
      if (!context.mounted) return true;
      if (refresh) ref.invalidate(accountDevicesProvider(sessionKey));
      if (done != null) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text(done)));
      }
      return true;
    }

    /// Every wider setting back to owner-only first, so pairing the same key
    /// again can't bring one back. Never in the way of the disconnect.
    Future<void> narrow(
      AccountApi api,
      Future<List<String>> Function() agents,
    ) async {
      final owner = ref.read(externalAccessWriterProvider);
      final relay = ref.read(agentSettingsRelayProvider);
      try {
        await () async {
          for (final agent in await agents()) {
            // Owner tools off first.
            await lowerToolLevelBeforeDisconnect(api, agent);
            if (owner == null) continue;
            await narrowExternalAgentAccess(
              relay,
              owner: owner,
              agentPubkey: agent,
            );
          }
        }().timeout(_narrowLimit);
      } catch (failure) {
        debugPrint('[Devices] no agents to narrow: $failure');
      }
    }

    Future<void> disconnect(
      String name,
      Future<List<String>> Function(AccountApi) agents,
      Future<void> Function(AccountApi) act,
    ) async {
      final ok = await confirmDestructive(
        context,
        title: context.l10n.settingsDevicesDisconnectTitle(name),
        body: context.l10n.settingsDevicesDisconnectBody(name),
        action: context.l10n.settingsDevicesDisconnect,
      );
      if (!ok) return;
      // On failure too: it may be gone already.
      final done = await run((api) async {
        await narrow(api, () => agents(api));
        await act(api);
      });
      if (!done && context.mounted) {
        ref.invalidate(accountDevicesProvider(sessionKey));
      }
    }

    Future<void> signOutDevice(AccountDevice device) async {
      if (device.isExternal) {
        return disconnect(
          device.name,
          // Read fresh: this list can miss an agent connected since.
          (api) async => [
            for (final fresh in await api.listDevices())
              if (fresh.id == device.id)
                for (final agent in fresh.agents) agent.pubkey,
          ],
          (api) => api.revokeDevice(device.id),
        );
      }
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
                  for (final (device, externals) in _nestExternal(list)) ...[
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
                    for (final external in externals)
                      for (final agent in external.agents)
                        _ExternalAgentRow(
                          agent: agent,
                          onOpen: (name) => unawaited(
                            showExternalAgentSheet(
                              context,
                              ref,
                              agentPubkey: agent.pubkey,
                              name: name,
                            ),
                          ),
                          onDisconnect: busy.value
                              ? null
                              : (name) => unawaited(
                                  disconnect(
                                    name,
                                    (_) async => [agent.pubkey],
                                    (api) => api.disconnectAgent(
                                      external.id,
                                      agent.pubkey,
                                    ),
                                  ),
                                ),
                        ),
                  ],
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
                          ref.invalidate(accountDevicesProvider(sessionKey)),
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
                if (agents) DisconnectAgentsRow(sessionKey: sessionKey),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

/// Each computer once, with the external devices whose agents run there:
/// those of the signed-in device on it, else its own.
List<(AccountDevice, List<AccountDevice>)> _nestExternal(
  List<AccountDevice> devices,
) {
  AccountDevice? hostOf(AccountDevice device) {
    // Without agents to list it needs its own row, for its Disconnect.
    if (!device.isExternal || device.agents.isEmpty) return null;
    for (final other in devices) {
      if (other.id == device.loginDeviceId) return other;
    }
    return null;
  }

  return [
    for (final device in devices)
      if (hostOf(device) == null)
        (
          device,
          [
            for (final other in devices)
              if (other.isExternal &&
                  (other.id == device.id || hostOf(other)?.id == device.id))
                other,
          ],
        ),
  ];
}

/// One external agent, under the row of the computer it runs on.
class _ExternalAgentRow extends ConsumerWidget {
  const _ExternalAgentRow({
    required this.agent,
    required this.onOpen,
    required this.onDisconnect,
  });

  final AccountDeviceAgent agent;

  /// Gets the agent's name.
  final ValueChanged<String> onOpen;

  /// Gets the agent's name.
  final ValueChanged<String>? onDisconnect;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final runtime = agent.runtime;
    final label = runtime == null
        ? context.l10n.settingsDevicesExternalAgent
        : context.l10n.settingsDevicesExternalAgentRuntime(runtime);
    final known = ref.watch(
      agentDirectoryDisplayNamesProvider.select(
        (names) => names[agent.pubkey.toLowerCase()],
      ),
    );
    final name = known ?? runtime ?? label;
    final callback = onDisconnect;
    return AppListRowRaw(
      key: Key('agent-row-${agent.pubkey}'),
      onTap: () => onOpen(name),
      leading: Icon(
        LucideIcons.bot,
        size: 24,
        color: context.colors.onSurfaceVariant,
      ),
      title: Text(label, style: context.textTheme.bodyLarge),
      subtitle: known == null || known == runtime
          ? null
          : Text(
              known,
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
      trailing: TextButton(
        key: Key('agent-disconnect-${agent.pubkey}'),
        onPressed: callback == null ? null : () => callback(name),
        child: Semantics(
          label: context.l10n.settingsDevicesDisconnectLabel(name),
          excludeSemantics: true,
          child: Text(context.l10n.settingsDevicesDisconnect),
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
          // An external device's agents have their own rows.
          if (!device.current && !device.isExternal)
            TextButton(
              key: Key('device-sign-out-${device.id}'),
              onPressed: onSignOut,
              child: Semantics(
                label: context.l10n.settingsDevicesSignOutLabel(device.name),
                excludeSemantics: true,
                child: Text(context.l10n.settingsDevicesSignOut),
              ),
            )
          else if (device.isExternal && device.agents.isEmpty)
            TextButton(
              key: Key('device-disconnect-${device.id}'),
              onPressed: onSignOut,
              child: Semantics(
                label: context.l10n.settingsDevicesDisconnectLabel(device.name),
                excludeSemantics: true,
                child: Text(context.l10n.settingsDevicesDisconnect),
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
        'external' => l10n.settingsDevicesExternalAgent,
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
