part of '../settings_page.dart';

class _NotificationsSection extends ConsumerWidget {
  const _NotificationsSection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    if (defaultTargetPlatform != TargetPlatform.iOS) {
      return const SizedBox.shrink();
    }
    final community = ref.watch(activeCommunityProvider).value;
    if (community == null) return const SizedBox.shrink();
    final capability = ref.watch(currentRelayPushDescriptorProvider);
    final hasCapability =
        !capability.isLoading &&
        !capability.hasError &&
        capability.value != null;
    final optOutPending =
        community.pushSubscriptionState.pendingTombstoneGeneration != null;
    if (!hasCapability &&
        !community.pushNotificationsEnabled &&
        !optOutPending) {
      return const SizedBox.shrink();
    }
    final authorization = ref.watch(kuzzPushAuthorizationStatusProvider);
    final status = authorization.value;
    final permissionUnavailable = authorization.hasError;
    final permissionDenied = status == KuzzPushAuthorizationStatus.denied;
    final showSettingsRecovery =
        optOutPending ||
        (community.pushNotificationsEnabled &&
            (permissionDenied || permissionUnavailable));
    final canToggle = hasCapability || community.pushNotificationsEnabled;
    final hasBuiltInServer = ref.watch(hasBuiltInServerProvider);
    final l10n = context.l10n;
    final subtitle = optOutPending
        ? l10n.settingsPushOptOutPending
        : !community.pushNotificationsEnabled
        ? hasBuiltInServer
              ? l10n.settingsPushOff
              : l10n.settingsPushOffCommunity
        : !hasCapability
        ? l10n.settingsPushUnsupported
        : switch (status) {
            KuzzPushAuthorizationStatus.notDetermined =>
              l10n.settingsPushWaitingPermission,
            KuzzPushAuthorizationStatus.denied => l10n.settingsPushDeniedInIos,
            KuzzPushAuthorizationStatus.authorized ||
            KuzzPushAuthorizationStatus.provisional ||
            KuzzPushAuthorizationStatus.ephemeral =>
              hasBuiltInServer
                  ? l10n.settingsPushOn
                  : l10n.settingsPushOnCommunity,
            null when authorization.isLoading =>
              l10n.settingsPushCheckingPermission,
            null => l10n.settingsPushPermissionUnknown,
          };

    return AppListCard(
      label: l10n.settingsNotificationsSection,
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const ValueKey('push-notifications-enabled'),
          icon: LucideIcons.bell,
          title: l10n.settingsPushNotifications,
          subtitle: subtitle,
          subtitleStyle: showSettingsRecovery
              ? context.textTheme.bodySmall?.copyWith(
                  color: context.colors.error,
                )
              : null,
          trailing: Switch.adaptive(
            value: community.pushNotificationsEnabled,
            onChanged: !canToggle
                ? null
                : (enabled) => unawaited(
                    ref
                        .read(communityListProvider.notifier)
                        .setPushNotificationsEnabled(community.id, enabled),
                  ),
          ),
          onTap: !canToggle
              ? null
              : () => unawaited(
                  ref
                      .read(communityListProvider.notifier)
                      .setPushNotificationsEnabled(
                        community.id,
                        !community.pushNotificationsEnabled,
                      ),
                ),
        ),
        if (showSettingsRecovery)
          AppListRow(
            key: const ValueKey('push-notifications-open-settings'),
            icon: LucideIcons.settings,
            title: l10n.settingsPushOpenIosSettings,
            onTap: () => unawaited(
              ref.read(kuzzPushNotificationSettingsOpenerProvider)(),
            ),
          ),
      ],
    );
  }
}
