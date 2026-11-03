part of '../settings_page.dart';

class _NotificationsSection extends HookConsumerWidget {
  const _NotificationsSection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final community = ref.watch(activeCommunityProvider).value;
    final setupError = useValueListenable(pushSetupError);
    final registrationError = useValueListenable(apnsRegistrationError);
    final unavailableOnPhone = useValueListenable(
      kuzzPushUnavailableOnThisPhone,
    );
    if (community == null) return const SizedBox.shrink();
    // Only this account's failure counts.
    final setupFailed =
        setupError?.communityId == community.id || registrationError != null;
    final android = androidPushActive;
    final availability = ref.watch(kuzzPushAvailabilityProvider);
    final enabled = community.pushNotificationsEnabled;
    final optOutPending =
        community.pushSubscriptionState.pendingTombstoneGeneration != null;
    final subscriptionsUnavailable =
        enabled &&
        availability == KuzzPushAvailability.available &&
        ref.watch(pushSubscriptionSyncProvider) ==
            KuzzPushSubscriptionSync.unavailable;
    final reachable =
        availability != KuzzPushAvailability.notInThisBuild &&
        availability != KuzzPushAvailability.signInMethodUnsupported;
    // The phone's permission matters only while push is on and offered.
    final authorization =
        enabled &&
            reachable &&
            availability != KuzzPushAvailability.serverUnsupported
        ? ref.watch(kuzzPushAuthorizationStatusProvider)
        : null;
    final status = authorization?.value;
    final permissionUnavailable = authorization?.hasError ?? false;
    final permissionDenied = status == KuzzPushAuthorizationStatus.denied;
    final leaseAccepted =
        community.pushSubscriptionState.authority ==
        KuzzPushLeaseSubscriptionAuthority.accepted;
    final hasBuiltInServer = ref.watch(hasBuiltInServerProvider);
    final l10n = context.l10n;
    final showSettingsRecovery =
        optOutPending ||
        (!subscriptionsUnavailable &&
            (permissionDenied || permissionUnavailable));
    // Push that is on can always be turned off; turning it on needs a
    // server that offers it.
    final canToggle = enabled || availability == KuzzPushAvailability.available;
    final subtitle = optOutPending
        ? l10n.settingsPushTurningOff
        : switch (availability) {
            KuzzPushAvailability.notInThisBuild =>
              l10n.settingsPushUnavailableBuild,
            KuzzPushAvailability.signInMethodUnsupported =>
              l10n.settingsPushUnavailableSignIn,
            KuzzPushAvailability.serverUnsupported =>
              l10n.settingsPushUnavailableServer,
            _ when !enabled =>
              hasBuiltInServer
                  ? l10n.settingsPushOff
                  : l10n.settingsPushOffCommunity,
            _ when unavailableOnPhone => l10n.settingsPushUnavailablePhone,
            _ when subscriptionsUnavailable =>
              l10n.settingsPushUnavailableAccount,
            _ when permissionDenied => l10n.settingsPushBlocked,
            _ when permissionUnavailable =>
              android
                  ? l10n.settingsPushPermissionUnknownAndroid
                  : l10n.settingsPushPermissionUnknown,
            _ when setupFailed => l10n.settingsPushSetupFailed,
            KuzzPushAvailability.checking => l10n.settingsPushSettingUp,
            _ => switch (status) {
              KuzzPushAuthorizationStatus.notDetermined =>
                android
                    ? l10n.settingsPushWaitingPermissionAndroid
                    : l10n.settingsPushWaitingPermission,
              null when authorization?.isLoading ?? false =>
                android
                    ? l10n.settingsPushCheckingPermissionAndroid
                    : l10n.settingsPushCheckingPermission,
              // "On" only once the server accepted this phone.
              _ when !leaseAccepted => l10n.settingsPushSettingUp,
              KuzzPushAuthorizationStatus.authorized ||
              KuzzPushAuthorizationStatus.provisional ||
              KuzzPushAuthorizationStatus.ephemeral =>
                hasBuiltInServer
                    ? l10n.settingsPushOn
                    : l10n.settingsPushOnCommunity,
              _ =>
                android
                    ? l10n.settingsPushPermissionUnknownAndroid
                    : l10n.settingsPushPermissionUnknown,
            },
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
            value: enabled,
            onChanged: !canToggle || optOutPending
                ? null
                : (value) => unawaited(
                    ref
                        .read(communityListProvider.notifier)
                        .setPushNotificationsEnabled(community.id, value),
                  ),
          ),
          onTap: !canToggle || optOutPending
              ? null
              : () => unawaited(
                  ref
                      .read(communityListProvider.notifier)
                      .setPushNotificationsEnabled(community.id, !enabled),
                ),
        ),
        if (showSettingsRecovery)
          AppListRow(
            key: const ValueKey('push-notifications-open-settings'),
            icon: LucideIcons.settings,
            title: l10n.settingsPushOpenSettings,
            onTap: () => unawaited(
              ref.read(kuzzPushNotificationSettingsOpenerProvider)(),
            ),
          ),
      ],
    );
  }
}
