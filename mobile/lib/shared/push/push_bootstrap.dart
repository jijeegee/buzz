import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../auth/default_community.dart';
import '../community/community.dart';
import '../community/community_provider.dart';
import '../relay/relay_provider.dart';
import '../relay/relay_session.dart';
import '../relay/signed_event_relay.dart';
import 'dev_push_lease.dart';
import 'push_bridge.dart';
import 'push_lease_revocation_outbox.dart';
import 'push_relay_capability_provider.dart';
import 'push_subscription.dart';

const _pushBootstrapRetryDelay = Duration(seconds: 5);
const _pushBootstrapMaxRetryDelay = Duration(minutes: 10);

/// One push setup step at a time. A failed attempt is retried after
/// [retryDelay], doubling while the same attempt keeps failing, up to
/// [maxRetryDelay]. Success or a different attempt starts over.
@visibleForTesting
class KuzzPushAttemptGate {
  KuzzPushAttemptGate({
    this.retryDelay = _pushBootstrapRetryDelay,
    this.maxRetryDelay = _pushBootstrapMaxRetryDelay,
  });

  final Duration retryDelay;
  final Duration maxRetryDelay;
  String? _attempt;
  Timer? _retryTimer;
  String? _failingAttempt;
  int _failures = 0;

  /// The delay chosen by the latest [failed].
  @visibleForTesting
  Duration? lastRetryDelay;

  bool tryBegin(String attempt) {
    if (_attempt == attempt) return false;
    _retryTimer?.cancel();
    _retryTimer = null;
    _attempt = attempt;
    return true;
  }

  void failed(String attempt, {required VoidCallback retry}) {
    if (_attempt != attempt) return;
    _attempt = null;
    _failures = _failingAttempt == attempt ? _failures + 1 : 1;
    _failingAttempt = attempt;
    final backoff = retryDelay * (1 << math.min(_failures - 1, 20));
    final delay = backoff > maxRetryDelay ? maxRetryDelay : backoff;
    lastRetryDelay = delay;
    _retryTimer?.cancel();
    _retryTimer = Timer(delay, () {
      _retryTimer = null;
      if (_attempt == null) retry();
    });
  }

  void retryAfter(
    String attempt, {
    required Duration delay,
    required VoidCallback retry,
  }) {
    if (_attempt != attempt) return;
    resetBackoff();
    _retryTimer?.cancel();
    _retryTimer = Timer(delay, () {
      _retryTimer = null;
      if (_attempt != attempt) return;
      _attempt = null;
      retry();
    });
  }

  void complete(String attempt) {
    if (_attempt != attempt) return;
    resetBackoff();
    _retryTimer?.cancel();
    _retryTimer = null;
    _attempt = null;
  }

  /// The running attempt succeeded but stays held; the next failure waits
  /// [retryDelay] again.
  void resetBackoff() {
    _failingAttempt = null;
    _failures = 0;
  }

  void dispose() => _retryTimer?.cancel();
}

@visibleForTesting
String kuzzPushPublicationAttemptKey({
  required String communityId,
  required String relayBaseUrl,
  required String token,
  required KuzzPushLeaseDescriptor descriptor,
  required List<KuzzPushSubscription> subscriptions,
}) => [
  communityId,
  relayBaseUrl,
  token,
  descriptor.executorKeyId,
  descriptor.executorPubkey,
  kuzzPushSubscriptionsFingerprint(subscriptions),
].join('|');

@visibleForTesting
bool kuzzPushLifecycleEnabled({
  required Community? community,
  required KuzzPushLeaseDescriptor? descriptor,
}) =>
    Env.pushGatewayConfigured &&
    community?.pushNotificationsEnabled == true &&
    descriptor != null;

/// Starts APNs registration when the active community opts in.
@visibleForTesting
class KuzzPushRegistrationBootstrap extends HookWidget {
  const KuzzPushRegistrationBootstrap({
    required this.shouldRegister,
    required this.attemptKey,
    required this.child,
    this.retrySignal = 0,
    this.startRegistration = startKuzzPushRegistration,
    super.key,
  });

  final bool shouldRegister;
  final String attemptKey;
  final Widget child;

  /// Changes on app resume; a failed registration is retried at once.
  final int retrySignal;
  final Future<void> Function() startRegistration;

  @override
  Widget build(BuildContext context) {
    final attemptGate = useMemoized(KuzzPushAttemptGate.new);
    final retry = useState(0);
    useEffect(() => attemptGate.dispose, const []);
    void scheduleRetry() => attemptGate.failed(
      attemptKey,
      retry: () {
        if (context.mounted) retry.value += 1;
      },
    );
    useEffect(() {
      // iOS reports a failed APNs registration later, outside the call.
      void onFailure() {
        if (apnsDeviceToken.value != null) return;
        scheduleRetry();
      }

      void onToken() {
        if (apnsDeviceToken.value != null) attemptGate.resetBackoff();
      }

      apnsRegistrationFailures.addListener(onFailure);
      apnsDeviceToken.addListener(onToken);
      return () {
        apnsRegistrationFailures.removeListener(onFailure);
        apnsDeviceToken.removeListener(onToken);
      };
    }, [attemptKey]);
    useEffect(() {
      if (!shouldRegister || !attemptGate.tryBegin(attemptKey)) return null;
      unawaited(() async {
        try {
          await startRegistration();
        } catch (error, stack) {
          pushSetupError.value = error.toString();
          scheduleRetry();
          debugPrint('Push registration bootstrap failed: $error');
          debugPrintStack(stackTrace: stack);
        }
      }());
      return null;
    }, [shouldRegister, attemptKey, retry.value, retrySignal]);
    return child;
  }
}

@visibleForTesting
Future<int> publishKuzzPushLeaseRecoverably({
  required Future<int> Function() reserveGeneration,
  required Future<void> Function(int generation) publish,
  required Future<bool> Function(int generation) markAccepted,
}) async {
  final generation = await reserveGeneration();
  await publish(generation);
  if (!await markAccepted(generation)) {
    throw StateError('A newer push lease superseded the published generation.');
  }
  return generation;
}

/// Starts the push lifecycle only after authenticated relay connectivity and a
/// push-capable NIP-11 descriptor are both present.
class KuzzPushBootstrap extends HookConsumerWidget {
  const KuzzPushBootstrap({required this.child, super.key});

  final Widget child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    useListenable(apnsDeviceToken);
    final publicationAttempt = useMemoized(KuzzPushAttemptGate.new);
    final tombstoneAttempt = useMemoized(KuzzPushAttemptGate.new);
    final publicationRetry = useState(0);
    final tombstoneRetry = useState(0);
    final resumes = useState(0);
    final revocationOutbox = ref.watch(kuzzPushLeaseRevocationOutboxProvider);
    final session = ref.watch(relaySessionProvider);
    final communities = ref.watch(communityListProvider).value ?? const [];
    final config = ref.watch(relayConfigProvider);
    final community = ref.watch(activeCommunityProvider).value;
    final memberPubkey = ref.watch(myPubkeyProvider);
    final descriptor = ref.watch(currentRelayPushDescriptorProvider).value;

    useEffect(() {
      final listener = AppLifecycleListener(
        onResume: () {
          _runRevocationOutbox(revocationOutbox.trigger);
          // A failed check of the server's push support, device token or
          // setup is retried now rather than after its backoff.
          if (ref.read(currentRelayPushDescriptorProvider).hasError) {
            ref.invalidate(currentRelayPushDescriptorProvider);
          }
          if (context.mounted) resumes.value += 1;
        },
      );
      _runRevocationOutbox(revocationOutbox.start);
      return listener.dispose;
    }, [revocationOutbox]);

    useEffect(() {
      if (session.status == SessionStatus.connected) {
        _runRevocationOutbox(revocationOutbox.trigger);
      }
      return null;
    }, [revocationOutbox, session.status]);

    useEffect(
      () => () {
        publicationAttempt.dispose();
        tombstoneAttempt.dispose();
      },
      const [],
    );

    useEffect(
      () {
        final pendingCommunities = communities
            .where(
              (candidate) =>
                  !candidate.pushNotificationsEnabled &&
                  candidate.pushSubscriptionState.pendingTombstoneGeneration !=
                      null &&
                  isServerAllowed(
                    candidate.relayUrl,
                    builtInOrigin: ref.read(defaultCommunityOriginProvider),
                  ),
            )
            .toList();
        if (session.status != SessionStatus.connected ||
            pendingCommunities.isEmpty) {
          return null;
        }
        const attempt = 'pending-tombstones';
        if (!tombstoneAttempt.tryBegin(attempt)) return null;
        unawaited(() async {
          try {
            Object? firstError;
            StackTrace? firstStack;
            for (final pendingCommunity in pendingCommunities) {
              try {
                await ref
                    .read(communityListProvider.notifier)
                    .retryPendingPushLeaseTombstone(
                      pendingCommunity.id,
                      advanceGeneration: true,
                    );
              } catch (error, stack) {
                firstError ??= error;
                firstStack ??= stack;
              }
            }
            if (firstError != null) {
              Error.throwWithStackTrace(firstError, firstStack!);
            }
            tombstoneAttempt.complete(attempt);
          } catch (error, stack) {
            tombstoneAttempt.failed(
              attempt,
              retry: () {
                if (context.mounted) tombstoneRetry.value += 1;
              },
            );
            debugPrint('Push lease tombstone retry failed: $error');
            debugPrintStack(stackTrace: stack);
          }
        }());
        return null;
      },
      [
        session.status,
        for (final candidate in communities)
          '${candidate.id}|${candidate.pushNotificationsEnabled}|'
              '${candidate.pushSubscriptionState.pendingTombstoneGeneration}',
        tombstoneRetry.value,
      ],
    );

    final token = apnsDeviceToken.value;
    useEffect(
      () {
        if (!_ready(session, config, community, memberPubkey) ||
            !kuzzPushLifecycleEnabled(
              community: community,
              descriptor: descriptor,
            ) ||
            token == null) {
          return null;
        }
        final activeCommunity = community!;
        final activeDescriptor = descriptor!;
        final state = activeCommunity.pushSubscriptionState;
        if (state.desired.isEmpty) return null;
        final attempt = kuzzPushPublicationAttemptKey(
          communityId: activeCommunity.id,
          relayBaseUrl: config.baseUrl,
          token: token,
          descriptor: activeDescriptor,
          subscriptions: state.desired,
        );
        if (!publicationAttempt.tryBegin(attempt)) return null;
        final relay = SignedEventRelay(
          session: ref.read(relaySessionProvider.notifier),
          nsec: config.nsec!,
        );
        unawaited(() async {
          try {
            final grant = await _publish(
              ref,
              config,
              activeCommunity,
              memberPubkey!,
              relay,
            );
            final renewInMilliseconds =
                grant.expiresAt * 1000 -
                DateTime.now().millisecondsSinceEpoch -
                const Duration(minutes: 5).inMilliseconds;
            pushSetupError.value = null;
            publicationAttempt.retryAfter(
              attempt,
              delay: Duration(
                milliseconds: renewInMilliseconds > 1000
                    ? renewInMilliseconds
                    : 1000,
              ),
              retry: () {
                if (context.mounted) publicationRetry.value += 1;
              },
            );
          } catch (error, stack) {
            pushSetupError.value = error.toString();
            publicationAttempt.failed(
              attempt,
              retry: () {
                if (context.mounted) publicationRetry.value += 1;
              },
            );
            debugPrint('Push lease bootstrap failed: $error');
            debugPrintStack(stackTrace: stack);
          }
        }());
        return null;
      },
      [
        session.status,
        config.baseUrl,
        community?.id,
        community?.pushNotificationsEnabled,
        community?.pushSubscriptionState,
        memberPubkey,
        descriptor,
        token,
        publicationRetry.value,
        resumes.value,
      ],
    );

    return KuzzPushRegistrationBootstrap(
      shouldRegister:
          _ready(session, config, community, memberPubkey) &&
          kuzzPushLifecycleEnabled(
            community: community,
            descriptor: descriptor,
          ),
      attemptKey: '${community?.id}|${config.baseUrl}',
      retrySignal: resumes.value,
      child: child,
    );
  }

  static bool _ready(
    SessionState session,
    RelayConfig config,
    Community? community,
    String? memberPubkey,
  ) =>
      session.status == SessionStatus.connected &&
      community != null &&
      config.nsec != null &&
      config.nsec!.isNotEmpty &&
      memberPubkey != null &&
      memberPubkey.isNotEmpty;

  static Future<KuzzPushEndpointGrant> _publish(
    WidgetRef ref,
    RelayConfig config,
    Community community,
    String memberPubkey,
    SignedEventRelay relay,
  ) async {
    final state = community.pushSubscriptionState;
    final desired = state.desired;
    final descriptor = await fetchKuzzPushLeaseDescriptor(config.baseUrl);
    final grant = await enrollKuzzPush(config.wsUrl, Env.pushGatewayUrl);
    await ref.read(communityListProvider.notifier).syncSnapshot();
    // Relay lease replacement and gateway delegation are independent state
    // machines. Subscription changes advance only the kind-30350 generation;
    // the opaque grant remains reusable until its own authority changes.
    final notifier = ref.read(communityListProvider.notifier);
    await publishKuzzPushLeaseRecoverably(
      reserveGeneration: () => notifier.reservePushLeaseGeneration(
        community.id,
        installationId: community.pushLeaseInstallationId,
      ),
      publish: (leaseGeneration) => publishKuzzDevPushLeaseThroughRelay(
        grant: grant,
        leaseInstallationId: community.pushLeaseInstallationId,
        leaseGeneration: leaseGeneration,
        descriptor: descriptor,
        nsec: config.nsec!,
        memberPubkey: memberPubkey,
        subscriptions: desired,
        relay: relay,
      ),
      markAccepted: (leaseGeneration) => notifier.markPushLeaseAccepted(
        community.id,
        subscriptions: desired,
        generation: leaseGeneration,
        installationId: community.pushLeaseInstallationId,
      ),
    );
    return grant;
  }
}

void _runRevocationOutbox(Future<void> Function() operation) {
  unawaited(
    operation().catchError((Object error, StackTrace stackTrace) {
      reportPushLeaseCleanupError(error, stackTrace);
    }),
  );
}
