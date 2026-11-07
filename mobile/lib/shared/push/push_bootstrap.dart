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
///
/// While a failed attempt waits out its backoff, [tryBegin] refuses to
/// start it again, however often its inputs are rebuilt. Only the timer or
/// an explicit [retryNow] (app resume, reconnect) cuts the wait short.
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
    if (waitingOutBackoff(attempt)) return false;
    _retryTimer?.cancel();
    _retryTimer = null;
    _attempt = attempt;
    return true;
  }

  /// Whether [attempt] failed and its retry timer has not fired yet.
  bool waitingOutBackoff(String attempt) =>
      _attempt == null && _retryTimer != null && _failingAttempt == attempt;

  /// An explicit retry: a failed attempt waiting out its backoff may start
  /// again now. The failure count stays, so if it fails again the next wait
  /// is longer. A running or held attempt is not affected.
  void retryNow() {
    if (_attempt != null || _retryTimer == null) return;
    _retryTimer!.cancel();
    _retryTimer = null;
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
    reset();
  }

  /// Drops whatever attempt is running, held or waiting out a backoff, so
  /// the next [tryBegin] starts over even with the same key. A running
  /// attempt that finishes later finds itself dropped and changes nothing.
  void reset() {
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

/// What a lease publication depends on. Lease bookkeeping (reserved and
/// accepted generations) is left out, so writing it never looks like a new
/// attempt. [userRetry] changes when the person turns push off or on.
/// [leaseEpoch] moves when a revocation jumps the generation, so the account
/// publishes again even if the switch away and back was never built here.
@visibleForTesting
String kuzzPushPublicationAttemptKey({
  required String communityId,
  required String relayBaseUrl,
  required String token,
  required KuzzPushLeaseDescriptor descriptor,
  required List<KuzzPushSubscription> subscriptions,
  int userRetry = 0,
  int leaseEpoch = 0,
}) => [
  communityId,
  relayBaseUrl,
  token,
  descriptor.origin,
  descriptor.executorKeyId,
  descriptor.executorPubkey,
  descriptor.transport,
  descriptor.maxLeaseTtlSeconds,
  descriptor.maxContentLength,
  descriptor.maxPlaintextLength,
  descriptor.maxEndpointLength,
  descriptor.maxStringLength,
  kuzzPushSubscriptionsFingerprint(subscriptions),
  userRetry,
  leaseEpoch,
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
    this.communityId,
    this.retrySignal = 0,
    this.startRegistration = startKuzzPushRegistration,
    super.key,
  });

  final bool shouldRegister;
  final String attemptKey;
  final Widget child;

  /// The account a failure is reported for.
  final String? communityId;

  /// Changes on app resume and reconnect; a failed registration is retried
  /// at once instead of after its backoff.
  final int retrySignal;
  final Future<void> Function() startRegistration;

  @override
  Widget build(BuildContext context) {
    final attemptGate = useMemoized(KuzzPushAttemptGate.new);
    final retry = useState(0);
    final seenRetrySignal = useRef(retrySignal);
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
      if (seenRetrySignal.value != retrySignal) {
        seenRetrySignal.value = retrySignal;
        attemptGate.retryNow();
      }
      if (!shouldRegister || !attemptGate.tryBegin(attemptKey)) return null;
      unawaited(() async {
        try {
          await startRegistration();
        } on KuzzPushUnavailableOnThisPhone {
          // Retrying can't help. The attempt stays held, so only a cold
          // start or turning push off and on tries again.
          debugPrint('Push registration: not available on this phone');
        } catch (error, stack) {
          pushSetupError.value = KuzzPushSetupError(
            communityId: communityId,
            message: error.toString(),
          );
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
/// Publishes the push lease for [community]; returns the gateway grant.
typedef KuzzPushLeasePublisher =
    Future<KuzzPushEndpointGrant> Function(
      WidgetRef ref,
      RelayConfig config,
      Community community,
      String memberPubkey,
      SignedEventRelay relay,
    );

class KuzzPushBootstrap extends HookConsumerWidget {
  const KuzzPushBootstrap({
    required this.child,
    this.publishLease = _publish,
    this.startRegistration = startKuzzPushRegistration,
    super.key,
  });

  final Widget child;

  /// Tests replace the relay and gateway calls.
  final KuzzPushLeasePublisher publishLease;
  final Future<void> Function() startRegistration;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    useListenable(apnsDeviceToken);
    final userRetry = useValueListenable(kuzzPushUserRetries);
    final publicationAttempt = useMemoized(KuzzPushAttemptGate.new);
    final tombstoneAttempt = useMemoized(KuzzPushAttemptGate.new);
    final publicationRetry = useState(0);
    final tombstoneRetry = useState(0);
    // Resumes and reconnects: explicit retries that skip a backoff.
    final resumes = useState(0);
    final reconnects = useRef(0);
    final previousStatus = useRef<SessionStatus?>(null);
    final revocationOutbox = ref.watch(kuzzPushLeaseRevocationOutboxProvider);
    final session = ref.watch(relaySessionProvider);
    if (previousStatus.value != session.status) {
      final previous = previousStatus.value;
      previousStatus.value = session.status;
      // Counted here rather than in an effect, so the effects below and the
      // registration see it in this same build.
      if (previous != null && session.status == SessionStatus.connected) {
        reconnects.value += 1;
        publicationAttempt.retryNow();
        tombstoneAttempt.retryNow();
      }
    }
    final retrySignal = resumes.value + reconnects.value;
    final communities = ref.watch(communityListProvider).value ?? const [];
    final config = ref.watch(relayConfigProvider);
    final community = ref.watch(activeCommunityProvider).value;
    // The account push is on for. A signed-out account reads as push off.
    final pushScope = community?.pushNotificationsEnabled == true
        ? community?.id
        : null;
    final previousPushScope = useRef<String?>(null);
    // Counts the times push stopped for an account: it signed out, push was
    // turned off, or another account (or none) became active.
    final pushScopeEnds = useRef(0);
    if (previousPushScope.value != pushScope) {
      if (previousPushScope.value != null) {
        // The held lease and registration belong to the session that ended.
        // Signing back in to the same account must publish again, even
        // though nothing in the attempt keys changed. A plain disconnect
        // keeps the scope, so it does not republish.
        pushScopeEnds.value += 1;
        publicationAttempt.reset();
      }
      previousPushScope.value = pushScope;
    }
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
          publicationAttempt.retryNow();
          tombstoneAttempt.retryNow();
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

    // A failure belongs to one account while push is on for it.
    useEffect(() {
      clearKuzzPushSetupErrorUnlessFor(pushScope);
      return null;
    }, [pushScope]);

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
          userRetry: userRetry,
          leaseEpoch:
              (state.generationCursor ?? 0) ~/ kuzzPushLeaseRevocationBlock,
        );
        if (!publicationAttempt.tryBegin(attempt)) return null;
        final relay = SignedEventRelay(
          session: ref.read(relaySessionProvider.notifier),
          nsec: config.nsec!,
        );
        unawaited(() async {
          try {
            final grant = await publishLease(
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
            clearKuzzPushSetupErrorFor(activeCommunity.id);
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
            pushSetupError.value = KuzzPushSetupError(
              communityId: activeCommunity.id,
              message: error.toString(),
            );
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
        userRetry,
        publicationRetry.value,
        retrySignal,
        pushScopeEnds.value,
      ],
    );

    return KuzzPushRegistrationBootstrap(
      shouldRegister:
          _ready(session, config, community, memberPubkey) &&
          kuzzPushLifecycleEnabled(
            community: community,
            descriptor: descriptor,
          ),
      attemptKey:
          '${community?.id}|${config.baseUrl}|$userRetry|'
          '${pushScopeEnds.value}',
      communityId: community?.id,
      retrySignal: retrySignal,
      startRegistration: startRegistration,
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
