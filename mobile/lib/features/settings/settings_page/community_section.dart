part of '../settings_page.dart';

class _CommunitySection extends ConsumerWidget {
  const _CommunitySection({required this.invitePageBuilder});

  final WidgetBuilder invitePageBuilder;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final roleAsync = ref.watch(currentCommunityRoleProvider);
    final canInvite =
        roleAsync.hasError || canManageCommunityInvites(roleAsync.value);
    final preference = ref.watch(communityThemeProvider);
    final hasBuiltInServer = ref.watch(hasBuiltInServerProvider);

    return AppListCard(
      label: hasBuiltInServer ? null : 'Community',
      verticalPadding: Grid.twelve,
      children: [
        if (canInvite)
          AppListRow(
            key: const ValueKey('settings-invite-row'),
            icon: LucideIcons.userPlus,
            title: hasBuiltInServer ? 'Invite people' : 'Invite to community',
            trailing: const _RowChevron(),
            onTap: () => Navigator.of(
              context,
            ).push(MaterialPageRoute<void>(builder: invitePageBuilder)),
          ),
        AppListRow(
          key: const ValueKey('community-theme-row'),
          icon: LucideIcons.palette,
          title: 'Theme',
          value: themeSelectionLabel(preference.theme, preference.mode),
          trailing: const _RowChevron(),
          onTap: () => Navigator.of(context).push(
            MaterialPageRoute<void>(builder: (_) => const ThemePickerPage()),
          ),
        ),
      ],
    );
  }
}
