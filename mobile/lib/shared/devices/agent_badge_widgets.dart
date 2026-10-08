import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../profile/user_cache_provider.dart';
import '../theme/theme.dart';
import '../widgets/avatar_image.dart';
import 'device_robot.dart';
import 'device_robot_icon.dart';

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
/// for everyone else. [child] unchanged when [badge] is `null`.
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
      AgentDeviceBadge(:final variant) => DeviceRobotAvatarBadge(
        robot: variant,
        badgeSize: badgeSize,
        child: child,
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
