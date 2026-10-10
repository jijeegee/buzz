import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../profile/user_cache_provider.dart';
import '../relay/relay.dart';
import '../theme/theme.dart';
import '../widgets/avatar_image.dart';
import 'agent_host_devices.dart';
import 'device_robot.dart';
import 'device_robot_icon.dart';

/// [agentBadge] for [agentPubkey], with the host device from the viewer's
/// own kind:30180 map and its robot from their kind:30181 choices. Watches
/// those only when the viewer owns the agent.
AgentBadge? watchAgentBadge(
  WidgetRef ref, {
  required String agentPubkey,
  required String? ownerPubkey,
  required String? viewerPubkey,
}) {
  final hostDeviceId =
      isAgentOwnerViewer(ownerPubkey: ownerPubkey, viewerPubkey: viewerPubkey)
      ? ref.watch(
          agentHostDevicesProvider.select(
            (devices) => devices[agentPubkey.toLowerCase()],
          ),
        )
      : null;
  final robotOverride = hostDeviceId == null
      ? null
      : ref.watch(
          deviceRobotOverridesProvider.select(
            (robots) => robots[hostDeviceId.toLowerCase()],
          ),
        );
  return agentBadge(
    hostDeviceId: hostDeviceId,
    ownerPubkey: ownerPubkey,
    viewerPubkey: viewerPubkey,
    robotOverride: robotOverride,
  );
}

/// Shows "Running on [device name]" (or "Running on this device") when
/// [child] (an owner's device robot) is long-pressed. Taps still reach the widgets around [child].
class AgentHostDeviceTooltip extends ConsumerWidget {
  const AgentHostDeviceTooltip({
    super.key,
    required this.deviceId,
    required this.child,
  });

  final String deviceId;
  final Widget child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final id = deviceId.toLowerCase();
    final name = ref.watch(
      agentHostDeviceNamesProvider.select((names) => names[id]),
    );
    final current = ref.watch(currentAccountDeviceIdProvider) == id;
    return Tooltip(
      message: agentHostDeviceLabel(name, current: current),
      triggerMode: TooltipTriggerMode.longPress,
      child: child,
    );
  }
}

/// An agent's robot glyph: the robot of the device it runs on for its owner
/// (with the device name on long-press), the default bot glyph otherwise.
///
/// [ownerPubkey] defaults to the cached profile's verified owner and
/// [viewerPubkey] to the signed-in identity.
class AgentRobotGlyph extends ConsumerWidget {
  const AgentRobotGlyph({
    super.key,
    required this.agentPubkey,
    this.ownerPubkey,
    this.viewerPubkey,
    this.size = 16,
    this.color,
    this.showDeviceName = true,
  });

  final String agentPubkey;
  final String? ownerPubkey;
  final String? viewerPubkey;
  final double size;

  /// Colour of the default bot glyph.
  final Color? color;

  /// Wrap the device robot in [AgentHostDeviceTooltip]. Off where a
  /// long-press already means something else (message bodies).
  final bool showDeviceName;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final pk = agentPubkey.toLowerCase();
    final owner =
        ownerPubkey ??
        ref.watch(userCacheProvider.select((cache) => cache[pk]?.ownerPubkey));
    final badge = owner == null
        ? null
        : watchAgentBadge(
            ref,
            agentPubkey: pk,
            ownerPubkey: owner,
            viewerPubkey: viewerPubkey ?? ref.watch(myPubkeyProvider),
          );
    if (badge case AgentDeviceBadge(:final deviceId, :final variant)) {
      final robot = DeviceRobotIcon(
        key: ValueKey('agent-device-robot-$pk'),
        variant: variant,
        size: size,
      );
      return showDeviceName
          ? AgentHostDeviceTooltip(deviceId: deviceId, child: robot)
          : robot;
    }
    return Icon(
      LucideIcons.bot,
      key: ValueKey('agent-default-robot-$pk'),
      size: size,
      color: color,
    );
  }
}

/// The owner's avatar as a small circle: on another account's agent it says
/// "belongs to this person" without passing for the person themselves.
class OwnerAvatarMark extends ConsumerWidget {
  const OwnerAvatarMark({
    super.key,
    required this.ownerPubkey,
    this.size = 18,
    this.showRobot = false,
  });

  final String ownerPubkey;
  final double size;

  /// Overlay a small robot glyph; used where the mark stands alone rather
  /// than on the agent's own avatar.
  final bool showRobot;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final owner =
        ref.watch(userCacheProvider.select((cache) => cache[ownerPubkey])) ??
        ref.read(userCacheProvider.notifier).get(ownerPubkey);
    final initial =
        owner?.initial ??
        (ownerPubkey.isNotEmpty ? ownerPubkey[0].toUpperCase() : '?');
    final avatar = AvatarImage(
      imageUrl: owner?.avatarUrl,
      radius: size / 2,
      backgroundColor: context.colors.primaryContainer,
      fallback: Text(
        initial,
        style: TextStyle(
          fontSize: size * 0.5,
          color: context.colors.onPrimaryContainer,
          fontWeight: FontWeight.w600,
        ),
      ),
    );
    final mark = SizedBox.square(
      key: ValueKey('agent-owner-mark-$ownerPubkey'),
      dimension: size,
      child: showRobot
          ? Stack(
              clipBehavior: Clip.none,
              children: [
                avatar,
                Positioned(
                  right: -size * 0.18,
                  bottom: -size * 0.18,
                  child: Container(
                    width: size * 0.6,
                    height: size * 0.6,
                    decoration: BoxDecoration(
                      color: context.colors.surface,
                      shape: BoxShape.circle,
                    ),
                    alignment: Alignment.center,
                    child: Icon(
                      LucideIcons.bot,
                      size: size * 0.48,
                      color: context.colors.onSurfaceVariant,
                    ),
                  ),
                ),
              ],
            )
          : avatar,
    );
    final name = owner?.displayName;
    return Semantics(
      label: name == null ? 'Agent owner' : 'Agent owned by $name',
      image: true,
      child: ExcludeSemantics(child: mark),
    );
  }
}

/// [child] (an agent avatar) with its [badge] in the bottom-right corner on a
/// surface-coloured disc: the device robot for the owner, the owner's avatar
/// for everyone else. [child] unchanged when [badge] is `null`. The owner
/// sees the device's name by long-pressing the badged avatar.
class AgentAvatarBadge extends StatelessWidget {
  const AgentAvatarBadge({
    super.key,
    required this.badge,
    required this.child,
    this.badgeSize = 18,
  });

  final AgentBadge? badge;
  final Widget child;
  final double badgeSize;

  @override
  Widget build(BuildContext context) {
    return switch (badge) {
      null => child,
      AgentDeviceBadge(:final deviceId, :final variant) =>
        AgentHostDeviceTooltip(
          deviceId: deviceId,
          child: DeviceRobotAvatarBadge(
            robot: variant,
            badgeSize: badgeSize,
            child: child,
          ),
        ),
      AgentOwnerBadge(:final ownerPubkey) => Stack(
        clipBehavior: Clip.none,
        children: [
          child,
          Positioned(
            right: -badgeSize * 0.25,
            bottom: -badgeSize * 0.25,
            child: Container(
              width: badgeSize,
              height: badgeSize,
              padding: EdgeInsets.all(badgeSize * 0.1),
              decoration: BoxDecoration(
                color: context.colors.surface,
                shape: BoxShape.circle,
              ),
              child: OwnerAvatarMark(
                ownerPubkey: ownerPubkey,
                size: badgeSize * 0.8,
              ),
            ),
          ),
        ],
      ),
    };
  }
}
