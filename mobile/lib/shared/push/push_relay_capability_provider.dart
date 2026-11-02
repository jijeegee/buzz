import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../community/community_provider.dart';
import '../relay/relay_provider.dart';
import '../relay/relay_session.dart';
import 'dev_push_lease.dart';
import 'push_bridge.dart';

typedef KuzzPushDescriptorFetcher =
    Future<KuzzPushLeaseDescriptor> Function(String relayBaseUrl);

final kuzzPushDescriptorFetcherProvider = Provider<KuzzPushDescriptorFetcher>(
  (ref) => fetchKuzzPushLeaseDescriptor,
);

/// The fully validated push capability advertised by the current relay.
///
/// An unconfigured artifact has no capability and never starts discovery.
/// Discovery fails closed. An absent or malformed NIP-11 push descriptor is
/// no capability, so no notification permission, APNs registration, gateway
/// enrollment, or relay lease can begin. A network or HTTP failure is an
/// error instead, retried with backoff and again on reconnect and resume.
final currentRelayPushDescriptorProvider =
    FutureProvider.autoDispose<KuzzPushLeaseDescriptor?>((ref) async {
      if (!Env.pushGatewayConfigured) return null;
      final session = ref.watch(relaySessionProvider);
      final config = ref.watch(relayConfigProvider);
      final community = ref.watch(activeCommunityProvider).value;
      final memberPubkey = ref.watch(myPubkeyProvider);
      if (community == null ||
          config.nsec == null ||
          config.nsec!.isEmpty ||
          memberPubkey == null ||
          memberPubkey.isEmpty) {
        return null;
      }
      // Not known until connected; this provider reruns when the session
      // changes, so it stays loading until then.
      if (session.status != SessionStatus.connected) {
        return Completer<KuzzPushLeaseDescriptor?>().future;
      }

      return discoverKuzzPushRelayCapability(
        config.baseUrl,
        fetchDescriptor: ref.read(kuzzPushDescriptorFetcherProvider),
      );
    });

/// The relay's push descriptor, or null when it advertises none or an
/// invalid one. Failing to reach the relay throws, so it can be retried.
/// Whether push can be offered to the active account, and if not, why.
enum KuzzPushAvailability {
  /// This build has no push, or this platform can't use it.
  notInThisBuild,

  /// The account signs in without a key on this phone (token sign-in).
  signInMethodUnsupported,

  /// Waiting for the connection or the server's answer, or retrying it.
  checking,

  /// The server offers no push this app can use.
  serverUnsupported,
  available,
}

final kuzzPushAvailabilityProvider = Provider.autoDispose<KuzzPushAvailability>(
  (ref) {
    if (!ref.watch(kuzzPushBuildSupportedProvider)) {
      return KuzzPushAvailability.notInThisBuild;
    }
    final community = ref.watch(activeCommunityProvider).value;
    if (community != null &&
        (community.tokenAuth ||
            community.nsec == null ||
            community.nsec!.isEmpty)) {
      return KuzzPushAvailability.signInMethodUnsupported;
    }
    final descriptor = ref.watch(currentRelayPushDescriptorProvider);
    if (!descriptor.hasError && descriptor.value != null) {
      return KuzzPushAvailability.available;
    }
    if (descriptor.isLoading || descriptor.hasError) {
      return KuzzPushAvailability.checking;
    }
    return KuzzPushAvailability.serverUnsupported;
  },
);

Future<KuzzPushLeaseDescriptor?> discoverKuzzPushRelayCapability(
  String relayBaseUrl, {
  required KuzzPushDescriptorFetcher fetchDescriptor,
}) async {
  try {
    return await fetchDescriptor(relayBaseUrl);
  } on FormatException catch (error, stackTrace) {
    debugPrint('Current relay does not advertise valid push: $error');
    debugPrintStack(stackTrace: stackTrace);
    return null;
  }
}

Future<void> startKuzzPushRegistrationIfCapable(
  KuzzPushLeaseDescriptor? descriptor, {
  required Future<void> Function() startRegistration,
}) async {
  if (descriptor == null) return;
  await startRegistration();
}
