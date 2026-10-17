import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import '../../shared/widgets/frosted_app_bar.dart';
import '../../shared/widgets/frosted_scaffold.dart';
import '../channels/channels_page.dart';
import '../profile/profile_avatar.dart';
import '../profile/profile_provider.dart';
import '../pulse/pulse_page.dart';
import '../search/search_page.dart';

/// The More tab: everything that is not a tab of its own — my profile and
/// Settings, the full channel list, Search and Pulse.
class MorePage extends ConsumerWidget {
  const MorePage({
    required this.settingsPageBuilder,
    this.onSettingsTransitionProgress,
    this.tabReselection,
    super.key,
  });

  final WidgetBuilder settingsPageBuilder;

  /// Reports the Settings entrance so Home can recede behind it, as the
  /// profile avatar on the channel list does.
  final ValueChanged<double>? onSettingsTransitionProgress;

  /// Notifies this page when its already-selected tab is tapped again.
  final ValueListenable<int>? tabReselection;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final profile = ref.watch(profileProvider).value;
    final name = profile?.displayName?.trim();
    final headerTitleStyle = context.textTheme.titleMedium?.copyWith(
      fontSize: 22,
      fontWeight: FontWeight.w600,
      color: navigationPrimaryForeground(context),
    );
    final topSectionHeight = frostedAppBarHeight(
      context,
      titleStyle: headerTitleStyle,
      bottomHeight: Grid.xxs,
    );

    void push(WidgetBuilder builder) {
      HapticFeedback.selectionClick();
      Navigator.of(context).push(MaterialPageRoute<void>(builder: builder));
    }

    void openSettings() {
      HapticFeedback.lightImpact();
      Navigator.of(context).push(
        SettingsPageRoute(
          builder: settingsPageBuilder,
          onTransitionProgress: onSettingsTransitionProgress ?? (_) {},
        ),
      );
    }

    Widget item({
      required Key key,
      required IconData icon,
      required String title,
      required VoidCallback onTap,
    }) => ListTile(
      key: key,
      leading: Icon(icon, size: 20, color: context.colors.onSurfaceVariant),
      title: Text(title),
      trailing: Icon(
        LucideIcons.chevronRight,
        size: 18,
        color: context.colors.onSurfaceVariant,
      ),
      onTap: onTap,
    );

    return FrostedScaffold(
      backgroundColor: context.colors.surface,
      appBar: FrostedAppBar(
        automaticallyImplyLeading: false,
        horizontalInset: Grid.gutter,
        showBottomDivider: true,
        bottomDividerOpacity: 0.07,
        title: Text('More', style: headerTitleStyle),
        titleStyle: headerTitleStyle,
        bottomHeight: Grid.xxs,
        bottom: const SizedBox.expand(),
      ),
      body: ListView(
        key: const ValueKey('more-list'),
        padding: EdgeInsets.only(
          top: topSectionHeight + Grid.xxs,
          bottom: MediaQuery.paddingOf(context).bottom + Grid.xs,
        ),
        children: [
          InkWell(
            key: const ValueKey('more-profile'),
            onTap: openSettings,
            child: Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: Grid.gutter,
                vertical: Grid.xs,
              ),
              child: Row(
                children: [
                  const ProfileAvatar(size: 48, showPresence: false),
                  const SizedBox(width: Grid.twelve),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          name == null || name.isEmpty ? 'Me' : name,
                          style: context.textTheme.titleMedium?.copyWith(
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                        Text(
                          'Profile and settings',
                          style: context.textTheme.bodySmall?.copyWith(
                            color: context.colors.onSurfaceVariant,
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
          const Divider(height: Grid.xs),
          item(
            key: const ValueKey('more-channels'),
            icon: LucideIcons.hash,
            title: 'Channels',
            onTap: () => push(
              (_) => ChannelsPage(
                settingsPageBuilder: settingsPageBuilder,
                onSettingsTransitionProgress: (_) {},
                showBackButton: true,
              ),
            ),
          ),
          item(
            key: const ValueKey('more-search'),
            icon: LucideIcons.search,
            title: 'Search',
            onTap: () => push((_) => const SearchPage()),
          ),
          item(
            key: const ValueKey('more-pulse'),
            icon: LucideIcons.zap,
            title: 'Pulse',
            onTap: () => push((_) => const PulsePage()),
          ),
          item(
            key: const ValueKey('more-settings'),
            icon: LucideIcons.settings,
            title: 'Settings',
            onTap: openSettings,
          ),
        ],
      ),
    );
  }
}
