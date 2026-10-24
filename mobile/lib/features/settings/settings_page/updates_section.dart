part of '../settings_page.dart';

enum _UpdateCheckView { idle, checking, upToDate, failed }

/// Settings › Updates: installed version, a manual check, and the update
/// when one is offered.
class _UpdatesSection extends HookConsumerWidget {
  const _UpdatesSection({required this.version, required this.buildNumber});

  final String? version;
  final String? buildNumber;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final update = ref.watch(appUpdateProvider);
    final checkView = useState(_UpdateCheckView.idle);
    final canCheck = appUpdatePlatform != null;
    final release = update.release;

    Future<void> check() async {
      checkView.value = _UpdateCheckView.checking;
      final result = await ref.read(appUpdateProvider.notifier).check();
      if (!context.mounted) return;
      checkView.value = switch (result) {
        AppUpdateCheck.available => _UpdateCheckView.idle,
        AppUpdateCheck.upToDate => _UpdateCheckView.upToDate,
        AppUpdateCheck.failed => _UpdateCheckView.failed,
      };
    }

    final versionText = version == null
        ? ''
        : (buildNumber == null || buildNumber!.isEmpty)
        ? 'v$version'
        : 'v$version ($buildNumber)';

    final String? checkSubtitle = switch (checkView.value) {
      _UpdateCheckView.checking => context.l10n.settingsCheckingForUpdates,
      _UpdateCheckView.upToDate => context.l10n.settingsUpToDate,
      _UpdateCheckView.failed => context.l10n.settingsUpdateCheckFailed,
      _UpdateCheckView.idle => null,
    };

    final String? installSubtitle = switch (update.phase) {
      AppUpdatePhase.downloading =>
        update.progress == null
            ? context.l10n.appUpdateDownloading
            : context.l10n.appUpdateDownloadingPercent(
                (update.progress! * 100).round(),
              ),
      AppUpdatePhase.failed => context.l10n.appUpdateFailed,
      _ => null,
    };

    return AppListCard(
      label: context.l10n.settingsUpdatesSection,
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const ValueKey('settings-version-row'),
          icon: LucideIcons.info,
          title: context.l10n.settingsInstalledVersion,
          value: versionText,
        ),
        if (canCheck && release != null)
          AppListRow(
            key: const ValueKey('settings-update-install-row'),
            icon: LucideIcons.download,
            title: context.l10n.appUpdateAvailable(release.version),
            subtitle: installSubtitle,
            trailing: const _RowChevron(),
            onTap: update.phase == AppUpdatePhase.downloading
                ? null
                : () => ref.read(appUpdateProvider.notifier).install(),
          )
        else if (canCheck)
          AppListRow(
            key: const ValueKey('settings-update-check-row'),
            icon: LucideIcons.refreshCw,
            title: context.l10n.settingsCheckForUpdates,
            subtitle: checkSubtitle,
            onTap: checkView.value == _UpdateCheckView.checking ? null : check,
          ),
      ],
    );
  }
}
