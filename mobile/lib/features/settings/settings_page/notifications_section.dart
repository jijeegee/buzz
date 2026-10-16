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
    final subtitle = optOutPending
        ? 'Waiting for relay confirmation; notifications may continue'
        : !community.pushNotificationsEnabled
        ? hasBuiltInServer
              ? 'Off'
              : 'Off for this community'
        : !hasCapability
        ? 'Push support unavailable; you can still turn notifications off'
        : switch (status) {
            KuzzPushAuthorizationStatus.notDetermined =>
              'Waiting for iOS notification permission',
            KuzzPushAuthorizationStatus.denied =>
              'Enabled in Kuzz, but disabled in iOS Settings',
            KuzzPushAuthorizationStatus.authorized ||
            KuzzPushAuthorizationStatus.provisional ||
            KuzzPushAuthorizationStatus.ephemeral =>
              hasBuiltInServer
                  ? 'Receive message notifications'
                  : 'Receive message notifications from this community',
            null when authorization.isLoading =>
              'Checking iOS notification permission',
            null => 'Enabled in Kuzz; iOS permission status unavailable',
          };

    return AppListCard(
      label: 'Notifications',
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const ValueKey('push-notifications-enabled'),
          icon: LucideIcons.bell,
          title: 'Push notifications',
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
            title: 'Open iOS Notification Settings',
            onTap: () => unawaited(
              ref.read(kuzzPushNotificationSettingsOpenerProvider)(),
            ),
          ),
      ],
    );
  }
}
