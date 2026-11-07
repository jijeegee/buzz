import 'dart:async';

import 'package:nostr/nostr.dart' as nostr;
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;

import '../community/community.dart';
import '../deeplink/deep_link.dart';
import '../relay/relay_provider.dart';
import '../relay/app_lifecycle_provider.dart';
import 'android_push.dart';
import 'android_push_enrollment.dart';
import 'push_snapshot.dart';
import 'push_thread_target.dart';

const _channel = MethodChannel('kuzz/push');

enum KuzzPushAuthorizationStatus {
  notDetermined,
  denied,
  authorized,
  provisional,
  ephemeral,
}

typedef KuzzPushAuthorizationStatusReader =
    Future<KuzzPushAuthorizationStatus> Function();
typedef KuzzPushNotificationSettingsOpener = Future<bool> Function();

/// Whether this build can offer push on this platform: it was made with a
/// push gateway and runs on iOS, or on Android with Android push built in.
/// Push starts on by default only where this holds. Tests override it.
final kuzzPushBuildSupportedProvider = Provider<bool>(
  (ref) =>
      Env.pushGatewayConfigured &&
      (androidPushActive ||
          (!kIsWeb && defaultTargetPlatform == TargetPlatform.iOS)),
);

final kuzzPushAuthorizationStatusReaderProvider =
    Provider<KuzzPushAuthorizationStatusReader>((ref) {
      return readKuzzPushAuthorizationStatus;
    });

final kuzzPushNotificationSettingsOpenerProvider =
    Provider<KuzzPushNotificationSettingsOpener>((ref) {
      return openKuzzPushNotificationSettings;
    });

final kuzzPushAuthorizationStatusProvider =
    AsyncNotifierProvider<
      KuzzPushAuthorizationStatusNotifier,
      KuzzPushAuthorizationStatus
    >(KuzzPushAuthorizationStatusNotifier.new);

class KuzzPushAuthorizationStatusNotifier
    extends AsyncNotifier<KuzzPushAuthorizationStatus> {
  @override
  Future<KuzzPushAuthorizationStatus> build() async {
    ref.listen(appLifecycleProvider, (previous, next) {
      if (previous != AppLifecycleState.resumed &&
          next == AppLifecycleState.resumed) {
        unawaited(refresh());
      }
    });
    // The person just answered the permission dialog.
    void onPrompt() => unawaited(refresh());
    kuzzPushPermissionPrompts.addListener(onPrompt);
    ref.onDispose(() => kuzzPushPermissionPrompts.removeListener(onPrompt));
    return ref.read(kuzzPushAuthorizationStatusReaderProvider)();
  }

  Future<void> refresh() async {
    state = const AsyncLoading<KuzzPushAuthorizationStatus>();
    state = await AsyncValue.guard(
      ref.read(kuzzPushAuthorizationStatusReaderProvider),
    );
  }
}

Future<KuzzPushAuthorizationStatus> readKuzzPushAuthorizationStatus() async {
  final android = androidPushActive;
  if (!android && defaultTargetPlatform != TargetPlatform.iOS) {
    return KuzzPushAuthorizationStatus.authorized;
  }
  final raw = android
      ? await readAndroidNotificationPermission()
      : await _channel.invokeMethod<String>('notificationAuthorizationStatus');
  return switch (raw) {
    'notDetermined' => KuzzPushAuthorizationStatus.notDetermined,
    'denied' => KuzzPushAuthorizationStatus.denied,
    'authorized' => KuzzPushAuthorizationStatus.authorized,
    'provisional' => KuzzPushAuthorizationStatus.provisional,
    'ephemeral' => KuzzPushAuthorizationStatus.ephemeral,
    _ => throw FormatException(
      'Native push bridge returned unknown authorization status: $raw',
    ),
  };
}

Future<bool> openKuzzPushNotificationSettings() async {
  if (androidPushActive) return openAndroidNotificationSettings();
  if (defaultTargetPlatform != TargetPlatform.iOS) return false;
  return await _channel.invokeMethod<bool>('openNotificationSettings') ?? false;
}

/// Latest APNs registration state, including callbacks replayed by iOS after
/// the Flutter method channel attaches. In Android push builds it carries
/// the FCM token instead (only tokens [fcmEndpoint] accepts), so a token
/// refresh re-enrolls and republishes the lease the same way.
final apnsDeviceToken = ValueNotifier<String?>(null);
final apnsRegistrationError = ValueNotifier<String?>(null);

/// Counts failed APNs registrations, so a repeat of the same failure is
/// still seen and retried.
final apnsRegistrationFailures = ValueNotifier<int>(0);

/// A failure to set up push for one account.
@immutable
class KuzzPushSetupError {
  const KuzzPushSetupError({required this.communityId, required this.message});

  final String? communityId;
  final String message;

  @override
  String toString() => message;
}

/// The latest failure to set up push (registration, enrollment or lease),
/// for the account it happened to. Cleared when that account's lease is
/// accepted, when push is turned off for it, and when another account (or
/// none) becomes active.
final pushSetupError = ValueNotifier<KuzzPushSetupError?>(null);

/// Clears [pushSetupError] unless it belongs to [communityId].
void clearKuzzPushSetupErrorUnlessFor(String? communityId) {
  final error = pushSetupError.value;
  if (error != null &&
      (communityId == null || error.communityId != communityId)) {
    pushSetupError.value = null;
  }
}

/// Clears [pushSetupError] when it belongs to [communityId].
void clearKuzzPushSetupErrorFor(String? communityId) {
  if (pushSetupError.value?.communityId == communityId) {
    pushSetupError.value = null;
  }
}

/// Counts the person's own retries: turning push on or off. A retry starts
/// setup over at once, without waiting out a backoff.
final kuzzPushUserRetries = ValueNotifier<int>(0);

/// This phone can't receive push: FCM gave a permanent reason (no Google
/// Play services, a wrong sender, no Firebase setup) or refused the token.
/// Setup stops until the next cold start or until the person turns push off
/// and on. Transient FCM errors never set this; they stay on the backoff.
final kuzzPushUnavailableOnThisPhone = ValueNotifier<bool>(false);

/// Counts answered notification permission dialogs, so the permission
/// status is read again right away.
final kuzzPushPermissionPrompts = ValueNotifier<int>(0);

/// The person turned push on or off for [communityId]. Starts setup over,
/// clears a stale failure for that account and lets the next registration
/// ask for the notification permission again.
void noteKuzzPushToggled(String communityId, {required bool enabled}) {
  clearKuzzPushSetupErrorFor(communityId);
  apnsRegistrationError.value = null;
  kuzzPushUnavailableOnThisPhone.value = false;
  _androidPermissionPromptRequested = enabled;
  kuzzPushUserRetries.value += 1;
}

final pushEndpointGrants = ValueNotifier<List<KuzzPushEndpointGrant>>([]);
final pushEndpointGrantError = ValueNotifier<String?>(null);

/// The most recent notification response waiting for app navigation.
///
/// Native iOS buffers cold-start responses until Dart asks for them. This
/// notifier also carries warm responses into the existing deep-link pipeline.
final pendingPushNotificationLink = ValueNotifier<MessageDeepLink?>(null);

/// Bumped when the Android wake posted a notification while the app runs,
/// so already-read messages can be swept from the tray again.
final pushNotificationsPosted = ValueNotifier<int>(0);

/// Message notifications in the tray (iOS: one per message; Android: one per
/// room, showing its newest message). Empty without a native bridge.
Future<List<Map<Object?, Object?>>> readDeliveredPushNotifications() async {
  final android = androidPushActive;
  if (!android && defaultTargetPlatform != TargetPlatform.iOS) return const [];
  try {
    final list = await (android ? androidPushChannel : _channel)
        .invokeListMethod<Object?>('deliveredMessageNotifications');
    return [
      for (final item in list ?? const [])
        if (item is Map) item,
    ];
  } on MissingPluginException {
    return const [];
  } on PlatformException {
    return const [];
  }
}

/// Removes tray notifications by the keys [readDeliveredPushNotifications] gave.
Future<void> removeDeliveredPushNotifications(List<String> keys) async {
  if (keys.isEmpty) return;
  final android = androidPushActive;
  if (!android && defaultTargetPlatform != TargetPlatform.iOS) return;
  try {
    await (android ? androidPushChannel : _channel).invokeMethod<void>(
      'removeDeliveredNotifications',
      keys,
    );
  } on MissingPluginException {
    // Flutter tests and builds without the push plugin.
  } on PlatformException {
    // Best effort: the notification stays until the next sweep.
  }
}

MessageDeepLink? _pushNotificationLink(Object? arguments) {
  if (arguments is! Map) return null;
  final eventId = arguments['eventId'];
  final communityId = arguments['communityId'];
  final channelId = arguments['channelId'];
  if (eventId is! String ||
      eventId.isEmpty ||
      communityId is! String ||
      communityId.isEmpty ||
      channelId is! String ||
      channelId.isEmpty) {
    return null;
  }
  final threadRootId = arguments['threadRootId'];
  return MessageDeepLink(
    communityId: communityId,
    channelId: channelId,
    messageId: eventId,
    // A thread reply opens its thread; a malformed id falls back to the room.
    threadRootId: isPushEventId(threadRootId) ? threadRootId as String : null,
  );
}

/// Pulls a notification response that arrived before the Flutter method
/// handler was installed.
Future<void> syncPendingKuzzPushNotificationResponse() async {
  final android = androidPushActive;
  if (!android && defaultTargetPlatform != TargetPlatform.iOS) return;
  try {
    final arguments = android
        ? await androidPushChannel.invokeMapMethod<dynamic, dynamic>(
            'takeLaunchPayload',
          )
        : await _channel.invokeMapMethod<dynamic, dynamic>(
            'takePendingNotificationResponse',
          );
    final link = _pushNotificationLink(arguments);
    if (link != null) pendingPushNotificationLink.value = link;
  } on MissingPluginException {
    // Flutter tests and non-Runner embeddings do not install the native bridge.
  }
}

/// Starts the independent iOS notification-authorization and APNs-registration
/// requests. Display authorization is intentionally not returned or persisted:
/// APNs registration and enrollment remain valid while display is denied.
///
/// On Android (KUZZ_ANDROID_PUSH builds) it asks for the notification
/// permission on Android 13+ at most once per user action: the first time
/// push is on in this run while the person was never asked, or right after
/// they turned push on. Retries only read the permission. Throws when no
/// usable token was obtained, so the caller retries, or
/// [KuzzPushUnavailableOnThisPhone] when retrying can't help.
Future<void> startKuzzPushRegistration() async {
  if (!Env.pushGatewayConfigured) return;
  if (androidPushActive) {
    if (kuzzPushUnavailableOnThisPhone.value) {
      throw const KuzzPushUnavailableOnThisPhone();
    }
    String? token;
    var permanent = false;
    try {
      await _askAndroidNotificationPermissionOnce();
      // FCM keeps working while notifications are blocked; the token is
      // asked for only here, after push was turned on.
      token = await androidPushChannel.invokeMethod<String>('token');
    } on MissingPluginException {
      // Flutter tests do not install the native plugin.
      return;
    } on PlatformException catch (error) {
      debugPrint('[AndroidPush] token failed: ${error.code} ${error.details}');
      permanent = _isPermanentAndroidTokenFailure(error);
    }
    if (token != null && _setAndroidPushToken(token)) return;
    // A refused token: the next one would be refused too.
    if (token != null) permanent = true;
    // Anything else (SERVICE_NOT_AVAILABLE, timeouts, IO) may clear up: the
    // relay being reachable says nothing about FCM. The caller retries on
    // its backoff, which tops out at one try every 10 minutes.
    if (permanent) {
      kuzzPushUnavailableOnThisPhone.value = true;
      throw const KuzzPushUnavailableOnThisPhone();
    }
    apnsRegistrationError.value ??= 'FCM token unavailable';
    throw StateError('FCM token unavailable');
  }
  if (defaultTargetPlatform != TargetPlatform.iOS) return;
  try {
    await _channel.invokeMethod<void>('startRegistration');
  } on MissingPluginException {
    // Flutter tests and non-Runner embeddings do not install the native bridge.
  }
}

/// Push can't work on this phone; retrying won't help.
class KuzzPushUnavailableOnThisPhone implements Exception {
  const KuzzPushUnavailableOnThisPhone();

  @override
  String toString() => 'Push is not available on this phone';
}

bool _androidPermissionPromptRequested = false;
bool _androidPermissionAskedThisRun = false;

/// FCM errors that retrying can't fix: no Firebase setup in this app, or no
/// Google Play services on the phone.
bool _isPermanentAndroidTokenFailure(PlatformException error) {
  if (error.code == 'not_configured') return true;
  final reason = '${error.message} ${error.details}';
  return reason.contains('MISSING_INSTANCEID_SERVICE') ||
      reason.contains('INVALID_SENDER');
}

/// Shows the system dialog only when the person never answered it and it
/// was not shown in this run yet, or right after they turned push on.
Future<void> _askAndroidNotificationPermissionOnce() async {
  final requested = _androidPermissionPromptRequested;
  _androidPermissionPromptRequested = false;
  try {
    final status = await readAndroidNotificationPermission();
    if (status == 'authorized') return;
    final neverAsked = status != 'denied' && !_androidPermissionAskedThisRun;
    if (!requested && !neverAsked) return;
    _androidPermissionAskedThisRun = true;
    await requestAndroidNotificationPermission();
    kuzzPushPermissionPrompts.value += 1;
  } on PlatformException catch (error) {
    // The token works without the permission; ask again on the next action.
    debugPrint('[AndroidPush] permission request failed: ${error.code}');
  }
}

/// Puts the registration state back as at a cold start.
@visibleForTesting
void debugResetKuzzPushRegistrationState() {
  _androidPermissionPromptRequested = false;
  _androidPermissionAskedThisRun = false;
  kuzzPushUnavailableOnThisPhone.value = false;
  pushSetupError.value = null;
  apnsRegistrationError.value = null;
}

class KuzzPushEndpointGrant {
  final String relayOrigin;
  final String relayPubkey;
  final String installationId;
  final String endpointGrant;
  final String endpointHash;
  final String appProfile;
  final int endpointEpoch;
  final int generation;
  final int expiresAt;

  const KuzzPushEndpointGrant({
    required this.relayOrigin,
    required this.relayPubkey,
    required this.installationId,
    required this.endpointGrant,
    required this.endpointHash,
    required this.appProfile,
    required this.endpointEpoch,
    required this.generation,
    required this.expiresAt,
  });

  factory KuzzPushEndpointGrant.fromMap(Map<dynamic, dynamic> map) {
    final generation = map['generation'] as int;
    return KuzzPushEndpointGrant(
      relayOrigin: map['relayOrigin'] as String,
      relayPubkey: map['relayPubkey'] as String,
      installationId: map['installationId'] as String,
      endpointGrant: map['endpointGrant'] as String,
      endpointHash: map['endpointHash'] as String,
      appProfile: map['appProfile'] as String,
      endpointEpoch: map['endpointEpoch'] as int,
      generation: generation,
      expiresAt: map['expiresAt'] as int,
    );
  }
}

Future<List<KuzzPushEndpointGrant>> readKuzzPushEndpointGrants() async {
  if (!Env.pushGatewayConfigured) return const [];
  if (androidPushActive) {
    final client = http.Client();
    try {
      final grants = [
        for (final record in await _androidEnrollment(client).grants())
          KuzzPushEndpointGrant.fromMap(record.toGrantMap()),
      ];
      pushEndpointGrants.value = grants;
      pushEndpointGrantError.value = null;
      return grants;
    } catch (error) {
      pushEndpointGrantError.value = error.toString();
      rethrow;
    } finally {
      client.close();
    }
  }
  if (defaultTargetPlatform != TargetPlatform.iOS) return const [];
  try {
    final raw = await _channel.invokeListMethod<dynamic>('endpointGrants', {
      'gatewayUrl': Env.pushGatewayUrl,
    });
    final grants = [
      for (final value in raw ?? const [])
        KuzzPushEndpointGrant.fromMap(value as Map<dynamic, dynamic>),
    ];
    pushEndpointGrants.value = grants;
    pushEndpointGrantError.value = null;
    return grants;
  } catch (error) {
    pushEndpointGrantError.value = error.toString();
    rethrow;
  }
}

/// Enrolls the endpoint. The caller rewrites the NSE snapshot afterward
/// (with the active account), which propagates NIP-11 `self` rotations even
/// when the opaque grant and accepted relay lease remain reusable.
Future<KuzzPushEndpointGrant> enrollKuzzPush(
  String relayUrl,
  String gatewayUrl,
) async {
  if (androidPushActive) {
    final token = apnsDeviceToken.value;
    if (token == null) throw StateError('No FCM token yet.');
    final client = http.Client();
    try {
      final record = await _androidEnrollment(
        client,
        gatewayUrl: gatewayUrl,
      ).enroll(token, relayUrl);
      await readKuzzPushEndpointGrants();
      // Android has no notification extension snapshot to refresh: the
      // wake reads the saved communities directly.
      return KuzzPushEndpointGrant.fromMap(record.toGrantMap());
    } finally {
      client.close();
    }
  }
  final raw = await _channel.invokeMapMethod<dynamic, dynamic>('enrollPush', {
    'relayUrl': relayUrl,
    'gatewayUrl': gatewayUrl,
  });
  if (raw == null) {
    throw StateError('Native push enrollment returned no grant.');
  }
  final grant = KuzzPushEndpointGrant.fromMap(raw);
  await readKuzzPushEndpointGrants();
  return grant;
}

/// Latest failure to export the community snapshot used by the iOS
/// notification service extension. Snapshot export is push enrichment and must
/// never gate authentication or community persistence.
final pushCommunitySnapshotError = ValueNotifier<String?>(null);
final pushLeaseCleanupError = ValueNotifier<String?>(null);

void reportPushCommunitySnapshotError(Object error, StackTrace stackTrace) {
  pushCommunitySnapshotError.value = error.toString();
  debugPrint('Push community snapshot export failed: $error');
  debugPrintStack(stackTrace: stackTrace);
}

void reportPushLeaseCleanupError(Object error, StackTrace stackTrace) {
  pushLeaseCleanupError.value = error.toString();
  debugPrint('Push lease cleanup failed: $error');
  debugPrintStack(stackTrace: stackTrace);
}

Future<void> registerKuzzPushCommunitySnapshot(
  List<Community> communities, {
  String? activeCommunityId,
}) => _registerKuzzPushCommunitySnapshot(
  communities,
  strict: false,
  activeCommunityId: activeCommunityId,
);

/// Writes the age-gate snapshot through a native path that must acknowledge
/// both the app-group store and signing-key update.
Future<void> registerKuzzPushCommunitySnapshotStrict(
  List<Community> communities, {
  required bool settleFence,
  String? activeCommunityId,
}) => _registerKuzzPushCommunitySnapshot(
  communities,
  strict: true,
  settleFence: settleFence,
  activeCommunityId: activeCommunityId,
);

/// Restores notification presentation without reading community storage.
Future<void> restoreAgeRestrictedKuzzNotifications() async {
  if (androidPushActive) return setAndroidPushAgeRestricted(false);
  if (defaultTargetPlatform != TargetPlatform.iOS) return;
  await _channel.invokeMethod<void>('restoreAgeRestrictedNotifications');
}

/// Removes notifications rendered before a confirmed age restriction.
Future<void> purgeAgeRestrictedKuzzNotifications() async {
  if (androidPushActive) return setAndroidPushAgeRestricted(true);
  if (defaultTargetPlatform != TargetPlatform.iOS) return;
  await _channel.invokeMethod<void>('purgeAgeRestrictedNotifications');
}

/// What the notification extension gets for a push-enabled [community].
/// [inbox] is [pushInboxEnabled]; tests pass it.
@visibleForTesting
KuzzPushCommunitySnapshot kuzzPushCommunitySnapshotFor(
  Community community, {
  bool inbox = pushInboxEnabled,
}) => KuzzPushCommunitySnapshot(
  id: community.id,
  name: community.name,
  relayUrl: community.relayUrl,
  pubkey: community.pubkey ?? pubkeyFromNsec(community.nsec),
  inboxToken: inbox && !community.signedOut ? community.pushInboxToken : null,
  subscriptions: community.pushSubscriptionState.authoritative,
);

Future<void> _registerKuzzPushCommunitySnapshot(
  List<Community> communities, {
  required bool strict,
  bool settleFence = false,
  String? activeCommunityId,
}) async {
  if (defaultTargetPlatform != TargetPlatform.iOS) return;
  try {
    final snapshots = [
      for (final community in communities)
        if (community.pushNotificationsEnabled)
          kuzzPushCommunitySnapshotFor(community),
    ];
    final signingKeys = <String, String>{};
    for (final community in communities) {
      // Token communities sign with no local key; a leftover legacy key
      // would pair the principal with a key it does not own.
      if (!community.pushNotificationsEnabled || community.tokenAuth) continue;
      final nsec = community.nsec;
      if (nsec == null || nsec.isEmpty) continue;
      try {
        final decoded = nostr.Nip19.decode(payload: nsec);
        if (decoded.prefix != nostr.Nip19Prefix.nsec ||
            decoded.data.length != 64) {
          continue;
        }
        final recorded = community.pubkey;
        if (recorded != null &&
            nostr.Keys(decoded.data).public != recorded.toLowerCase()) {
          continue;
        }
        signingKeys[community.id] = decoded.data;
      } catch (_) {
        // Native storage is fail-closed; malformed keys are never exported.
      }
    }
    await _channel.invokeMethod<void>(
      strict ? 'syncAgeGatePushSnapshot' : 'syncPushSnapshot',
      {
        'section': 'communities',
        'communities': [for (final snapshot in snapshots) snapshot.toJson()],
        'signingKeys': signingKeys,
        kuzzPushActiveCommunityKey: ?activeCommunityId,
        if (strict) 'settleFence': settleFence,
      },
    );
  } on MissingPluginException {
    if (strict) rethrow;
    // Flutter tests and non-Runner embeddings do not install the native bridge.
  }
}

void installKuzzPushMethodHandler() {
  if (androidPushActive) {
    androidPushChannel.setMethodCallHandler(handleAndroidPushCall);
  }
  _channel.setMethodCallHandler((call) async {
    switch (call.method) {
      case 'apnsTokenChanged':
        final args = call.arguments;
        if (args is Map) {
          final token = args['token'];
          if (token is String && token.isNotEmpty) {
            apnsDeviceToken.value = token;
            apnsRegistrationError.value = null;
          }
        }
        return null;
      case 'apnsRegistrationFailed':
        final args = call.arguments;
        final message = args is Map ? args['message'] : null;
        apnsRegistrationError.value = message is String && message.isNotEmpty
            ? message
            : 'APNs registration failed';
        apnsRegistrationFailures.value += 1;
        debugPrint('APNs registration failed: ${apnsRegistrationError.value}');
        return null;
      case 'notificationOpened':
        final link = _pushNotificationLink(call.arguments);
        if (link == null) return 'ignored';
        pendingPushNotificationLink.value = link;
        return 'handled';
      default:
        throw MissingPluginException('Unknown kuzz/push method ${call.method}');
    }
  });
}

/// Builds the Android gateway enrollment; tests replace it.
@visibleForTesting
AndroidPushEnrollment Function(http.Client client, String gatewayUrl)
debugAndroidPushEnrollmentFactory = (client, gatewayUrl) =>
    AndroidPushEnrollment(gatewayUrl: gatewayUrl, client: client);

AndroidPushEnrollment _androidEnrollment(
  http.Client client, {
  String? gatewayUrl,
}) =>
    debugAndroidPushEnrollmentFactory(client, gatewayUrl ?? Env.pushGatewayUrl);

/// Takes a new FCM token unless it is refused (N11: over 512 bytes).
/// Returns whether it was taken.
bool _setAndroidPushToken(String token) {
  if (fcmEndpoint(token) == null) {
    apnsRegistrationError.value = 'FCM token refused';
    return false;
  }
  apnsDeviceToken.value = token;
  apnsRegistrationError.value = null;
  return true;
}

/// Calls from the Android push plugin to the app's engine.
@visibleForTesting
Future<Object?> handleAndroidPushCall(MethodCall call) async {
  switch (call.method) {
    case 'tokenRefreshed':
      // S9: FCM rotated the token. A known token changes nothing; a new
      // one changes the endpoint hash, so the bootstrap enrolls it afresh
      // and republishes the lease.
      final args = call.arguments;
      final token = args is Map ? args['token'] : null;
      if (token is! String || token.isEmpty) return 'ignored';
      if (apnsDeviceToken.value == null) {
        // Not asked for yet: push is off, so nothing to re-enroll.
        return 'ignored';
      }
      _setAndroidPushToken(token);
      return 'handled';
    case 'notificationOpened':
      final link = _pushNotificationLink(call.arguments);
      if (link == null) return 'ignored';
      pendingPushNotificationLink.value = link;
      return 'handled';
    case 'notificationPosted':
      pushNotificationsPosted.value += 1;
      return 'handled';
    default:
      throw MissingPluginException(
        'Unknown kuzz/android_push method ${call.method}',
      );
  }
}
