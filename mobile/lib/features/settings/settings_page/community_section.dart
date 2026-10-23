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
    final language = ref.watch(localePreferenceProvider);
    final deviceLanguage = resolveAppLocale(
      View.of(context).platformDispatcher.locales,
    );

    return AppListCard(
      label: hasBuiltInServer
          ? context.l10n.settingsAppearanceSection
          : context.l10n.settingsCommunitySection,
      verticalPadding: Grid.twelve,
      children: [
        if (canInvite)
          AppListRow(
            key: const ValueKey('settings-invite-row'),
            icon: LucideIcons.userPlus,
            title: hasBuiltInServer
                ? context.l10n.settingsInvitePeople
                : context.l10n.settingsInviteToCommunity,
            trailing: const _RowChevron(),
            onTap: () => Navigator.of(
              context,
            ).push(MaterialPageRoute<void>(builder: invitePageBuilder)),
          ),
        AppListRow(
          key: const ValueKey('community-theme-row'),
          icon: LucideIcons.palette,
          title: context.l10n.settingsTheme,
          value: themeSelectionLabel(preference.theme, preference.mode),
          trailing: const _RowChevron(),
          onTap: () => Navigator.of(context).push(
            MaterialPageRoute<void>(builder: (_) => const ThemePickerPage()),
          ),
        ),
        AppListRow(
          key: const ValueKey('settings-language-row'),
          icon: LucideIcons.languages,
          title: context.l10n.settingsLanguage,
          trailing: const _RowChevron(),
          onTap: () => showLanguagePickerSheet(
            context: context,
            selected: language,
            deviceLocale: deviceLanguage,
            onSelected: ref.read(localePreferenceProvider.notifier).select,
          ),
        ),
      ],
    );
  }
}
