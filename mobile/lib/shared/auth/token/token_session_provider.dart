import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;

import '../../app_update/app_update.dart';
import '../../community/community_provider.dart';
import '../../device_key/device_key.dart';
import '../../device_key/device_proof_client.dart';
import 'auth_api.dart';
import 'install_id_store.dart';
import 'refresh_token_store.dart';
import 'relay_origin.dart';
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
  (ref) =>
      SecureRefreshTokenStore(keyPrefix: 'kuzz.auth.key-backup.refresh.v1'),
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

/// The token session controller for one canonical relay origin.
///
/// Kept alive for the app's lifetime so its refresh timer keeps running
/// while no widget watches it; disposed with the container.
final tokenSessionControllerProvider =
    Provider.family<TokenSessionController, String>((ref, origin) {
      final controller = TokenSessionController(
        origin: origin,
        api: AuthApi(
          origin: origin,
          client: ref.watch(authHttpClientProvider),
          deviceKey: DeviceKey.instance,
        ),
        store: ref.watch(refreshTokenStoreProvider),
        launcher: ref.watch(webAuthLauncherProvider),
        accountPicker: ref.watch(googleAccountPickerProvider),
        clock: ref.watch(sessionClockProvider),
        timerFactory: ref.watch(sessionTimerFactoryProvider),
        deviceName: ref.watch(authDeviceNameProvider),
        installIds: ref.watch(installIdStoreProvider),
      );
      ref.onDispose(controller.dispose);
      return controller;
    });

final keyBackupSessionControllerProvider =
    Provider.family<TokenSessionController, String>((ref, origin) {
      final controller = TokenSessionController(
        origin: origin,
        api: AuthApi(
          origin: origin,
          client: ref.watch(authHttpClientProvider),
          deviceKey: DeviceKey.instance,
        ),
        store: ref.watch(keyBackupRefreshTokenStoreProvider),
        launcher: ref.watch(webAuthLauncherProvider),
        accountPicker: ref.watch(googleAccountPickerProvider),
        clock: ref.watch(sessionClockProvider),
        timerFactory: ref.watch(sessionTimerFactoryProvider),
        deviceName: ref.watch(authDeviceNameProvider),
        installIds: ref.watch(installIdStoreProvider),
      );
      ref.onDispose(controller.dispose);
      return controller;
    });

/// UI-facing state of one origin's token session. Building it starts the
/// restore (stored refresh → rotation) automatically.
class TokenSessionNotifier extends Notifier<TokenSessionState> {
  TokenSessionNotifier(this.origin, {this.keyBackup = false});

  final String origin;
  final bool keyBackup;

  @override
  TokenSessionState build() {
    final controller = ref.watch(
      keyBackup
          ? keyBackupSessionControllerProvider(origin)
          : tokenSessionControllerProvider(origin),
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
        ? keyBackupSessionControllerProvider(origin)
        : tokenSessionControllerProvider(origin),
  );
}

final tokenSessionProvider =
    NotifierProvider.family<TokenSessionNotifier, TokenSessionState, String>(
      TokenSessionNotifier.new,
    );

final keyBackupSessionProvider =
    NotifierProvider.family<TokenSessionNotifier, TokenSessionState, String>(
      (origin) => TokenSessionNotifier(origin, keyBackup: true),
    );

/// Canonical origin of the active community when Google holds its key
/// (signed in with Google on this device), otherwise `null`. Its account
/// endpoints are called with the Google session that signed it in.
final activeGoogleKeyOriginProvider = Provider<String?>((ref) {
  final community = ref.watch(
    activeCommunityProvider.select((value) {
      final active = value.value;
      return active == null ||
              active.tokenAuth ||
              active.googleBackupAccountId == null
          ? null
          : active.relayUrl;
    }),
  );
  if (community == null) return null;
  try {
    return normalizeRelayOrigin(community);
  } on FormatException {
    return null;
  }
});

/// Canonical origin of the active community when it uses token auth,
/// otherwise `null` (legacy nsec community or none).
final activeTokenOriginProvider = Provider<String?>((ref) {
  final community = ref.watch(
    activeCommunityProvider.select((value) {
      final active = value.value;
      return active == null || !active.tokenAuth ? null : active.relayUrl;
    }),
  );
  if (community == null) return null;
  try {
    return normalizeRelayOrigin(community);
  } on FormatException {
    return null;
  }
});
