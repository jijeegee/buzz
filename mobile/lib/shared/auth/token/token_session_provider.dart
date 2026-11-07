import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart' show AppLifecycleState;
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;

import '../../app_update/app_update.dart';
import '../auth_provider.dart';
import '../default_community.dart';
import '../../community/community.dart';
import '../../community/community_provider.dart';
import '../../relay/app_lifecycle_provider.dart';
import '../../device_key/device_key.dart';
import '../../device_key/device_proof_client.dart';
import 'auth_api.dart';
import 'install_id_store.dart';
import 'refresh_token_store.dart';
import 'session_key.dart';
import 'token_session.dart';
import 'google_account_picker.dart';
import 'web_auth_launcher.dart';

/// HTTP client for `/auth/*` and NIP-11 detection. Account calls carry this
/// install's device proof ([DeviceProofClient]) and name this build
/// ([AppClientHeaderClient]). Override in tests.
final authHttpClientProvider = Provider<http.Client>((ref) {
  final client = AppClientHeaderClient(DeviceProofClient(http.Client()));
  ref.onDispose(client.close);
  return client;
});

/// Durable refresh-token storage. Override in tests.
final refreshTokenStoreProvider = Provider<RefreshTokenStore>(
  (ref) => SecureRefreshTokenStore(),
);

/// This install's stable device id (see [InstallIdStore]).
final installIdStoreProvider = Provider<InstallIdStore>(
  (ref) => InstallIdStore(),
);

/// Custody grants cannot overwrite a pre-existing token messaging session.
final keyBackupRefreshTokenStoreProvider = Provider<RefreshTokenStore>(
  (ref) => SecureRefreshTokenStore(
    keyPrefix: SecureRefreshTokenStore.keyBackupKeyPrefix,
  ),
);

/// System-browser launcher for OIDC. Override in tests.
final webAuthLauncherProvider = Provider<WebAuthLauncher>(
  (ref) => const FlutterWebAuth2Launcher(),
);

/// The phone's own Google account picker (Android), or `null` for the
/// browser only. Override in tests.
final googleAccountPickerProvider = Provider<GoogleAccountPicker?>(
  (ref) => platformGoogleAccountPicker(),
);

/// Clock for token expiry. Override in tests.
final sessionClockProvider = Provider<SessionClock>((ref) => DateTime.now);

/// Timer factory for refresh scheduling. Override in tests.
final sessionTimerFactoryProvider = Provider<SessionTimerFactory>(
  (ref) => Timer.new,
);

/// Device label shown in the relay's device list.
final authDeviceNameProvider = Provider<String>(
  (ref) => switch (defaultTargetPlatform) {
    TargetPlatform.iOS => 'Kuzz for iOS',
    TargetPlatform.android => 'Kuzz for Android',
    _ => 'Kuzz mobile',
  },
);

/// The token session controller of one account.
///
/// Kept alive for the app's lifetime so its refresh timer keeps running
/// while no widget watches it; disposed with the container.
final tokenSessionControllerProvider =
    Provider.family<TokenSessionController, SessionKey>((ref, key) {
      final controller = TokenSessionController(
        key: key,
        api: AuthApi(
          origin: key.origin,
          client: ref.watch(authHttpClientProvider),
          deviceKey: DeviceKey.instance,
        ),
        store: ref.watch(refreshTokenStoreProvider),
        clock: ref.watch(sessionClockProvider),
        timerFactory: ref.watch(sessionTimerFactoryProvider),
        canResumeQuietly: (principal) =>
            _showsAccount(ref.read(activeCommunityProvider).value, key),
      );
      ref.onDispose(controller.dispose);
      return controller;
    });

/// Whether [active] is the token community of exactly [key]: then its stored
/// content may be drawn before the first rotation
/// (TokenSessionStatus.resuming).
bool _showsAccount(Community? active, SessionKey key) =>
    active != null &&
    active.tokenAuth &&
    !active.signedOut &&
    SessionKey.ofCommunity(active) == key;

final keyBackupSessionControllerProvider =
    Provider.family<TokenSessionController, SessionKey>((ref, key) {
      final controller = TokenSessionController(
        key: key,
        api: AuthApi(
          origin: key.origin,
          client: ref.watch(authHttpClientProvider),
          deviceKey: DeviceKey.instance,
        ),
        store: ref.watch(keyBackupRefreshTokenStoreProvider),
        clock: ref.watch(sessionClockProvider),
        timerFactory: ref.watch(sessionTimerFactoryProvider),
      );
      ref.onDispose(controller.dispose);
      return controller;
    });

/// The controller provider holding [community]'s session under [key].
Provider<TokenSessionController> sessionControllerProviderOf(
  Community community,
  SessionKey key,
) => community.tokenAuth
    ? tokenSessionControllerProvider(key)
    : keyBackupSessionControllerProvider(key);

/// Starts one sign-in attempt at an origin (see
/// [TokenSessionController.attempt]). The caller owns the attempt: it either
/// hands it to the account or discards it, and then disposes it.
typedef TokenSignInAttemptFactory =
    TokenSessionController Function(String origin);

final tokenSignInAttemptProvider = Provider<TokenSignInAttemptFactory>(
  (ref) =>
      (origin) => TokenSessionController.attempt(
        origin: origin,
        api: AuthApi(
          origin: SessionKey(origin, '').origin,
          client: ref.read(authHttpClientProvider),
          deviceKey: DeviceKey.instance,
        ),
        launcher: ref.read(webAuthLauncherProvider),
        accountPicker: ref.read(googleAccountPickerProvider),
        clock: ref.read(sessionClockProvider),
        timerFactory: ref.read(sessionTimerFactoryProvider),
        deviceName: ref.read(authDeviceNameProvider),
        installIds: ref.read(installIdStoreProvider),
      ),
);

/// UI-facing state of one account's token session. Building it starts the
/// restore (stored refresh → rotation) automatically.
class TokenSessionNotifier extends Notifier<TokenSessionState> {
  TokenSessionNotifier(this.sessionKey, {this.keyBackup = false});

  final SessionKey sessionKey;
  final bool keyBackup;

  @override
  TokenSessionState build() {
    final controller = ref.watch(
      keyBackup
          ? keyBackupSessionControllerProvider(sessionKey)
          : tokenSessionControllerProvider(sessionKey),
    );
    final removeListener = controller.addListener((next) => state = next);
    ref.onDispose(removeListener);
    if (controller.state.status == TokenSessionStatus.restoring) {
      scheduleMicrotask(() => unawaited(controller.restore()));
    }
    return controller.state;
  }

  TokenSessionController get controller => ref.read(
    keyBackup
        ? keyBackupSessionControllerProvider(sessionKey)
        : tokenSessionControllerProvider(sessionKey),
  );
}

final tokenSessionProvider =
    NotifierProvider.family<
      TokenSessionNotifier,
      TokenSessionState,
      SessionKey
    >(TokenSessionNotifier.new);

final keyBackupSessionProvider =
    NotifierProvider.family<
      TokenSessionNotifier,
      TokenSessionState,
      SessionKey
    >((key) => TokenSessionNotifier(key, keyBackup: true));

/// The session of the active community when Google holds its key (signed in
/// with Google on this device), otherwise `null`. Its account endpoints are
/// called with the Google session that signed it in.
final activeGoogleKeySessionProvider = Provider<SessionKey?>(
  (ref) => ref.watch(
    activeCommunityProvider.select((value) {
      final active = value.value;
      return active == null || active.tokenAuth
          ? null
          : SessionKey.ofCommunity(active);
    }),
  ),
);

/// The session of the active community when it uses token auth, otherwise
/// `null` (legacy nsec community or none).
final activeTokenSessionProvider = Provider<SessionKey?>(
  (ref) => ref.watch(
    activeCommunityProvider.select((value) {
      final active = value.value;
      return active == null || !active.tokenAuth
          ? null
          : SessionKey.ofCommunity(active);
    }),
  ),
);

/// Keeps every signed-in account's session alive, not only the active one.
/// Watch it from the app root.
///
/// Restores each account's controller, the active account first and then the
/// others one at a time; from there each keeps its own refresh timer. Timers
/// do not run in the background, so on app resume any account near expiry is
/// refreshed in the same order.
///
/// An account that is not on screen has no gate to show its session ended, so
/// it is signed out here, for session-ended codes only: that deletes its key
/// and messages, which a passing failure must never do.
final accountSessionKeeperProvider = Provider<void>((ref) {
  final rows = ref.watch(communityListProvider);
  final active = ref.watch(activeCommunityProvider);
  if (!rows.hasValue || !active.hasValue) return;
  final activeId = active.value?.id;
  final builtInOrigin = ref.watch(defaultCommunityOriginProvider);
  final signedIn = [
    for (final community in rows.value!)
      if (!community.signedOut &&
          isServerAllowed(community.relayUrl, builtInOrigin: builtInOrigin))
        if (SessionKey.ofCommunity(community) case final key?)
          (
            id: community.id,
            provider: sessionControllerProviderOf(community, key),
          ),
  ];
  final ordered = [
    for (final account in signedIn)
      if (account.id == activeId) account.provider,
    for (final account in signedIn)
      if (account.id != activeId) account.provider,
  ];

  for (final account in signedIn) {
    if (account.id == activeId) continue;
    void signOutIfEnded(TokenSessionState session) {
      if (session.status != TokenSessionStatus.signedOut ||
          !refreshEndedCodes.contains(session.endedCode)) {
        return;
      }
      unawaited(
        ref
            .read(authProvider.notifier)
            .signOutCommunity(account.id, deviceOnly: true)
            .catchError((Object error) {
              debugPrint('Could not sign out an ended account: $error');
            }),
      );
    }

    final controller = ref.read(account.provider);
    ref.onDispose(controller.addListener(signOutIfEnded));
    signOutIfEnded(controller.state);
  }

  // The controllers are read before the first wait: this provider rebuilds
  // whenever the list or active row changes, and that must not end a pass
  // halfway through the accounts.
  Future<void> forEachAccount(
    Future<void> Function(TokenSessionController controller) action,
  ) async {
    final controllers = [for (final account in ordered) ref.read(account)];
    for (final controller in controllers) {
      try {
        await action(controller);
      } catch (error) {
        debugPrint('Could not refresh an account session: $error');
      }
    }
  }

  ref.listen(appLifecycleProvider, (_, next) {
    if (next != AppLifecycleState.resumed) return;
    unawaited(
      forEachAccount(
        (controller) => controller.state.status == TokenSessionStatus.restoring
            ? controller.restore()
            : controller.refreshIfExpiring(),
      ),
    );
  });

  // Only a controller nothing has restored yet does any work here, so the
  // list changing (push state, renames) costs nothing.
  unawaited(
    forEachAccount((controller) async {
      if (controller.state.status == TokenSessionStatus.restoring) {
        await controller.restore();
      }
    }),
  );
});
