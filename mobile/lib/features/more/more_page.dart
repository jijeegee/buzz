import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import '../profile/profile_avatar.dart';
import '../profile/profile_provider.dart';

/// The More tab's menu: a small popup above the tab bar, not a page of its
/// own: my profile on top, then Settings.
Future<void> showMoreMenu(
  BuildContext context, {
  required VoidCallback onOpenProfile,
  required VoidCallback onOpenSettings,
  required double bottomClearance,
}) {
  HapticFeedback.selectionClick();
  return showGeneralDialog<void>(
    context: context,
    barrierDismissible: true,
    barrierLabel: 'Close menu',
    barrierColor: Colors.black12,
    transitionDuration: const Duration(milliseconds: 120),
    pageBuilder: (dialogContext, _, _) => SafeArea(
      child: Align(
        alignment: Alignment.bottomRight,
        child: Padding(
          padding: EdgeInsets.only(
            right: Grid.gutter,
            bottom: bottomClearance + Grid.xs,
          ),
          child: _MoreMenu(
            onOpenProfile: () {
              Navigator.of(dialogContext).pop();
              onOpenProfile();
            },
            onOpenSettings: () {
              Navigator.of(dialogContext).pop();
              onOpenSettings();
            },
          ),
        ),
      ),
    ),
    transitionBuilder: (_, animation, _, child) => FadeTransition(
      opacity: animation,
      child: ScaleTransition(
        alignment: Alignment.bottomRight,
        scale: Tween(begin: 0.96, end: 1.0).animate(animation),
        child: child,
      ),
    ),
  );
}

class _MoreMenu extends ConsumerWidget {
  const _MoreMenu({required this.onOpenProfile, required this.onOpenSettings});

  final VoidCallback onOpenProfile;
  final VoidCallback onOpenSettings;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final profile = ref.watch(profileProvider).value;
    final name = profile?.displayName?.trim();
    return Material(
      key: const ValueKey('more-menu'),
      color: context.colors.surfaceContainerHigh,
      elevation: 6,
      borderRadius: BorderRadius.circular(16),
      clipBehavior: Clip.antiAlias,
      child: SizedBox(
        width: 240,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            InkWell(
              key: const ValueKey('more-profile'),
              onTap: onOpenProfile,
              child: Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: Grid.sm,
                  vertical: Grid.xs,
                ),
                child: Row(
                  children: [
                    const ProfileAvatar(size: 36, showPresence: false),
                    const SizedBox(width: Grid.twelve),
                    Expanded(
                      child: Text(
                        name == null || name.isEmpty ? 'Me' : name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: context.textTheme.titleSmall?.copyWith(
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
            const Divider(height: 1),
            ListTile(
              key: const ValueKey('more-settings'),
              dense: true,
              leading: Icon(
                LucideIcons.settings,
                size: 20,
                color: context.colors.onSurfaceVariant,
              ),
              title: const Text('Settings'),
              onTap: onOpenSettings,
            ),
          ],
        ),
      ),
    );
  }
}
