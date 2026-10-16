import 'dart:async';
import 'shared/read_aloud/read_aloud_message.dart';

import 'package:app_badge_plus/app_badge_plus.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';

import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'features/age_gate/age_restriction_page.dart';
import 'features/age_gate/age_signal_provider.dart';
import 'features/activity/activity_provider.dart';
import 'features/activity/inbox_local_state_provider.dart';
import 'features/activity/inbox_read_state.dart';
import 'features/channels/unread_badge/unread_badge_provider.dart';
import 'features/home/home_page.dart';
import 'features/invites/invite_create_page.dart';
import 'features/invites/invite_join_provider.dart';
import 'features/channels/agent_activity/observer_subscription.dart';
import 'features/channels/channel_detail_page.dart';
import 'features/channels/deep_link_dispatcher.dart';
import 'features/channels/voice_note_recording.dart';
import 'features/profile/user_status_cache_provider.dart';
import 'features/profile/settings_profile_header.dart';
import 'features/profile/profile_edit_page.dart';
import 'features/profile/profile_text_editor.dart';
import 'features/settings/settings_page.dart';
import 'features/sign_in/token_session_gate.dart';
import 'features/sign_in/token_sign_in_page.dart';
import 'features/sign_in/signed_out_page.dart';
import 'shared/auth/auth.dart';
import 'shared/auth/token/token.dart';
import 'shared/community/add_community_route.dart';
import 'shared/deeplink/pending_deep_link_provider.dart';
import 'shared/devices/agent_host_devices.dart';
import 'shared/emoji/emoji_burst.dart';
import 'shared/push/push_subscription_provider.dart';
import 'shared/push/push_relay_capability_provider.dart';
import 'shared/relay/relay.dart';
import 'shared/read_state/read_state_provider.dart';
import 'shared/theme/theme.dart';
import 'shared/widgets/buzz_loading_indicator.dart';

/// App-level bridge from invite joining to the channels feature.
///
/// A new member is not put in any public channel (#general,
/// #welcome-everyone): on a shared server those hold strangers, so members
/// join them from the channel directory instead. Setup only checks that the
/// community and identity that started it are still active.
class MobileInviteJoinRecovery implements InviteJoinRecovery {
  final bool Function() _isScopeCurrent;

  /// Creates recovery fenced to the scope that started it.
  MobileInviteJoinRecovery({bool Function()? isScopeCurrent})
    : _isScopeCurrent = isScopeCurrent ?? _alwaysCurrent;

  @override
  Future<String?> ensureStarterChannels() async {
    if (!_isScopeCurrent()) {
      throw StateError('Active community changed during invite recovery');
    }
    return null;
  }

  static bool _alwaysCurrent() => true;
}

/// Builds one fresh, identity-scoped invite recovery against the app container.
InviteJoinRecovery buildMobileInviteJoinRecovery(
  Ref ref,
  InviteJoinRecoveryScope scope,
) {
  final expectedConfig = ref.read(relayConfigProvider);
  if (expectedConfig.baseUrl != scope.relayHttpOrigin ||
      expectedConfig.nsec != scope.nsec) {
    throw StateError('Active community changed before invite recovery');
  }

  return MobileInviteJoinRecovery(
    isScopeCurrent: () {
      final currentConfig = ref.read(relayConfigProvider);
      return currentConfig.baseUrl == scope.relayHttpOrigin &&
          currentConfig.nsec == scope.nsec;
    },
  );
}

/// App-shell projection that joins Activity state for the Home navigation.
///
/// This belongs at the composition root because it deliberately aggregates
/// Activity feature providers for a sibling navigation surface.
final _unreadInboxItemCountProvider = Provider<int>((ref) {
  final readState = ref.watch(readStateProvider);
  if (!readState.isReady) return 0;

  final localState = ref.watch(inboxLocalStateProvider);
  final items = ref.watch(inboxItemsProvider);
  return items
      .where(
        (item) => !isInboxItemDone(
          item,
          markerOf: readState.effectiveTimestamp,
          localUnreadOverrides: localState.unreadIds,
          localDoneSet: localState.doneIds,
        ),
      )
      .length;
});

final _mobileRootNavigatorKey = GlobalKey<NavigatorState>();

class App extends HookConsumerWidget {
  const App({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final ageSignalState = ref.watch(ageSignalProvider);
    final communityTheme = ageSignalState != AgeSignalState.restricted
        ? ref.watch(communityThemeProvider)
        : defaultCommunityTheme;
    final themeMode = communityTheme.mode;
    final accentIndex = effectiveAccentIndex(
      communityTheme.theme,
      communityTheme.accent,
    );
    final schemeName = communityTheme.theme;
    final authState = ref.watch(authProvider);

    useEffect(() {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        unawaited(ref.read(ageSignalProvider.notifier).request());
      });
      return null;
    }, const []);

    final resolved = resolveSchemes(schemeName, themeMode);
    final lightScheme = applyAccent(resolved.light, accentIndex);
    final darkScheme = applyAccent(resolved.dark, accentIndex);
    // Light/Dark modes pin the brightness; System leaves it null so Flutter
    // follows the OS across the selected theme and its pair.
    final effectiveMode = resolved.forcedMode ?? themeMode;

    // Derive the gradient from the themes that produced each color scheme.
    // This keeps fallbacks and pinned brightness changes aligned with the
    // rendered palette rather than the raw persisted selection.
    final buzzLightGradient = buzzTopSectionGradient(
      resolved.lightTheme?.name ?? '',
      lightScheme.brightness,
    );
    final buzzDarkGradient = buzzTopSectionGradient(
      resolved.darkTheme?.name ?? '',
      darkScheme.brightness,
    );

    // Eagerly initialize websocket session and lifecycle observer when
    // authenticated. These providers connect and manage the websocket.
    var hasUnreadInbox = false;
    if (ageSignalState != AgeSignalState.restricted &&
        authState.value?.status == AuthStatus.authenticated) {
      ref.watch(relaySessionProvider);
      // Keep the observer subscription alive without rebuilding the app root:
      // every agent telemetry frame emits new state, and watching it here
      // rebuilt MaterialApp (and every Theme dependent) once per frame.
      ref.listen(observerRelayProvider, (_, _) {});
      ref.watch(appLifecycleProvider);
      ref.watch(userStatusCacheProvider);
      ref.listen(deviceRobotAutoAssignProvider, (_, _) {});
      if (ref.watch(activeCommunityProvider).value?.pushNotificationsEnabled ==
              true &&
          ref.watch(currentRelayPushDescriptorProvider).value != null) {
        ref.watch(pushSubscriptionSyncProvider);
      }
      hasUnreadInbox = ref.watch(_unreadInboxItemCountProvider) > 0;
    }

    // Start listening for buzz:// links immediately (even pre-auth) so a
    // cold-start link survives until the authenticated UI can dispatch it.
    ref.watch(pendingDeepLinkProvider);

    void applyBadge(UnreadBadgeState state) {
      if (state.highPriorityCount > 0) {
        AppBadgePlus.updateBadge(state.highPriorityCount);
      } else if (state.generalUnreadCount > 0) {
        AppBadgePlus.updateBadge(1);
      } else {
        AppBadgePlus.updateBadge(0);
      }
    }

    useEffect(() {
      if (ageSignalState != AgeSignalState.restricted) {
        applyBadge(ref.read(unreadBadgeProvider));
      } else {
        AppBadgePlus.updateBadge(0);
      }
      return null;
    }, [ageSignalState]);
    if (ageSignalState != AgeSignalState.restricted) {
      ref.listen<UnreadBadgeState>(unreadBadgeProvider, (_, next) {
        applyBadge(next);
      });
    }

    return MaterialApp(
      navigatorKey: _mobileRootNavigatorKey,
      navigatorObservers: [voiceNoteRouteObserver, readAloudRouteObserver],
      title: 'Buzz',
      theme: AppTheme.light(
        colorScheme: lightScheme,
        topSectionGradient: buzzLightGradient,
      ),
      darkTheme: AppTheme.dark(
        colorScheme: darkScheme,
        topSectionGradient: buzzDarkGradient,
      ),
      themeMode: effectiveMode,
      routes: {addCommunityRouteName: (_) => const TokenSignInPage()},
      // Above the navigator, so an age restriction cannot be bypassed by a
      // route that was pushed while the store signal request was in flight.
      builder: (context, child) => switch (ageSignalState) {
        AgeSignalState.restricted => const AgeRestrictionPage(),
        _ => AppMarkdownTheme(
          child: MobileHuddleShell(
            navigatorKey: _mobileRootNavigatorKey,
            child: EmojiBurstOverlay(child: child ?? const SizedBox.shrink()),
          ),
        ),
      },
      home: authState.when(
        loading: () => const _SplashScreen(),
        error: (_, _) => const TokenSignInPage(),
        data: (state) => switch (state.status) {
          AuthStatus.authenticated => DeepLinkDispatcher(
            child: _TokenGateIfNeeded(
              child: HomePage(
                settingsPageBuilder: _buildSettingsPage,
                hasUnreadInbox: hasUnreadInbox,
              ),
            ),
          ),
          _ => const DeepLinkDispatcher(
            dispatchMessageLinks: false,
            child: _OnboardingHome(),
          ),
        },
      ),
    );
  }
}

Widget _buildSettingsPage(BuildContext context) => const _SettingsPageContent();

class _SettingsPageContent extends ConsumerWidget {
  const _SettingsPageContent();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return SettingsPage(
      profileHeader: const SettingsProfileHeader(),
      profileEditPageBuilder: (_) =>
          const ProfileEditPage(startInPhotoEditor: true),
      onEditDisplayName: showProfileDisplayNameEditor,
      onEditProfileDescription: showProfileDescriptionEditor,
      invitePageBuilder: (_) => const CommunityInvitePage(),
    );
  }
}

/// Token communities only reach Home with a live token session; legacy
/// (nsec) communities pass straight through.
class _TokenGateIfNeeded extends ConsumerWidget {
  const _TokenGateIfNeeded({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final origin = ref.watch(activeTokenOriginProvider);
    if (origin == null) return child;
    return TokenSessionGate(
      key: ValueKey(origin),
      origin: origin,
      child: child,
    );
  }
}

/// Google creates or restores a signing key on any first device.
class _OnboardingHome extends ConsumerWidget {
  const _OnboardingHome();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final auth = ref.watch(authProvider).value;
    final community = auth?.community;
    if (community?.signedOut == true) {
      return SignedOutPage(
        community: community!,
        cleanupError: auth?.logoutError,
      );
    }
    return TokenSignInPage(
      defaultCommunity: community == null,
      backupOrigin: community?.relayUrl,
    );
  }
}

class _SplashScreen extends StatelessWidget {
  const _SplashScreen();

  @override
  Widget build(BuildContext context) {
    return const Scaffold(
      body: Center(
        child: BuzzLoadingIndicator(size: 56, semanticLabel: 'Starting Buzz'),
      ),
    );
  }
}
