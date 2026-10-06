import 'dart:async';

import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../community/community.dart';
import '../community/community_provider.dart';
import '../push/push_bridge.dart';
import '../push/push_subscription.dart';
import 'token/relay_origin.dart';

enum AuthStatus { unknown, unauthenticated, authenticated }

class AuthState {
  final AuthStatus status;
  final Community? community;

  const AuthState({required this.status, this.community});
}

/// Restores the active community without making connectivity load-bearing.
/// The relay session owns connection recovery after startup.
class AuthNotifier extends AsyncNotifier<AuthState> {
  @override
  Future<AuthState> build() async {
    // Read from storage directly — NOT from community providers.
    // Watching community providers here would create a circular dependency
    // because authenticateWithCommunity() writes to those providers.
    final storage = ref.read(communityStorageProvider);
    final communities = await storage.loadAll();
    if (communities.isEmpty) {
      await syncCommunitySnapshot(ref, communities);
      return const AuthState(status: AuthStatus.unauthenticated);
    }

    var activeId = await storage.loadActiveId();
    while (communities.isNotEmpty) {
      final active =
          activeId != null && communities.any((w) => w.id == activeId)
          ? communities.firstWhere((w) => w.id == activeId)
          : communities.first;
      await storage.saveActiveId(active.id);

      // Token communities authenticate through their relay token session
      // (see `TokenSessionGate`); legacy ones need a usable local key.
      if (active.tokenAuth || _hasValidNsec(active.nsec)) {
        await syncCommunitySnapshot(ref, communities);
        return AuthState(status: AuthStatus.authenticated, community: active);
      }

      // Preserve the pubkey boundary for Google recovery when the local
      // secure key is missing. Onboarding can restore only that same identity.
      if (active.googleBackupAccountId != null && active.pubkey != null) {
        await syncCommunitySnapshot(ref, communities);
        return AuthState(status: AuthStatus.unauthenticated, community: active);
      }

      await storage.remove(active.id);
      communities.removeWhere((community) => community.id == active.id);
      activeId = null;
      ref.invalidate(communityListProvider);
      ref.invalidate(activeCommunityProvider);
    }

    await storage.clearActiveId();
    await syncCommunitySnapshot(ref, communities);
    return const AuthState(status: AuthStatus.unauthenticated);
  }

  /// Authenticate with a community. Saves it and switches to it.
  /// Writes to storage directly to avoid circular dependency with community
  /// providers.
  Future<void> authenticateWithCommunity(Community community) {
    return ref.read(communityTransitionProvider).runExclusive(() async {
      await ref.read(communityTransitionProvider).run();
      final storage = ref.read(communityStorageProvider);
      await storage.save(community);
      await storage.saveActiveId(community.id);
      await syncStoredCommunitySnapshot(ref);

      // Invalidate community providers so other consumers pick up the new data.
      ref.invalidate(communityListProvider);
      ref.invalidate(activeCommunityProvider);

      state = AsyncData(
        AuthState(status: AuthStatus.authenticated, community: community),
      );
    });
  }

  /// Record a completed token sign-in for [relayUrl] and switch to it.
  ///
  /// Reuses the stored community for the same relay origin (marking it as
  /// token-authenticated and setting [principalId] as its identity),
  /// otherwise creates one.
  ///
  /// A reused legacy community's push lease was published by its `nsec`, so
  /// its revocation is durably journaled with that key first (AGENTS.md
  /// rule 1; a journal failure propagates and changes nothing). Only then is
  /// the community rewritten once, without the key and with push off
  /// (rule 5), so no state pairs the principal with a key it does not own.
  Future<void> authenticateWithTokenSession({
    required String relayUrl,
    required String principalId,
  }) async {
    final origin = normalizeRelayOrigin(relayUrl);
    final existing = (await ref.read(communityStorageProvider).loadAll())
        .where((community) => _sameOrigin(community.relayUrl, origin))
        .firstOrNull;
    if (existing?.googleBackupAccountId != null) {
      throw StateError(
        'This community uses a Google-backed signing key. '
        'Token sign-in cannot replace its identity. Use Google key recovery.',
      );
    }
    final legacyKey = existing?.nsec;
    var revocationJournaled = false;
    if (existing != null && legacyKey != null && legacyKey.isNotEmpty) {
      revocationJournaled = await ref.read(
        communityPushLeaseRevocationEnqueuerProvider,
      )(existing);
    }
    final community = existing != null
        ? existing.copyWith(
            tokenAuth: true,
            pubkey: principalId,
            nsec: legacyKey == null ? existing.nsec : null,
            pushNotificationsEnabled: legacyKey == null
                ? existing.pushNotificationsEnabled
                : false,
            pushSubscriptionState: legacyKey == null
                ? existing.pushSubscriptionState
                : const BuzzPushLeaseSubscriptionState.desired(),
          )
        : Community.create(
            name: Community.nameFromUrl(origin),
            relayUrl: origin,
            pubkey: principalId,
            tokenAuth: true,
          );
    await authenticateWithCommunity(community);
    if (revocationJournaled) {
      unawaited(
        ref
            .read(communityPushLeaseRevocationTriggerProvider)()
            .catchError(reportPushLeaseCleanupError),
      );
    }
  }

  /// Remove the active community. A token community's device session is
  /// revoked on the relay first; when the relay cannot be reached this throws
  /// `TokenSessionException` and removes nothing. [deviceOnly] skips the
  /// relay (see `CommunityListNotifier.removeCommunity`).
  Future<void> signOut({bool deviceOnly = false}) {
    return () async {
      final storage = ref.read(communityStorageProvider);
      await ref
          .read(communityListProvider.notifier)
          .removeActiveCommunityForSignOut(deviceOnly: deviceOnly);

      // Community removal already persisted the outbox, deleted credentials,
      // selected the next active community, and removed NSE state. Authentication
      // only needs to publish the truthful resulting account state.
      final remaining = await storage.loadAll();
      ref.invalidate(activeCommunityProvider);
      if (remaining.isEmpty) {
        state = const AsyncData(AuthState(status: AuthStatus.unauthenticated));
        return;
      }
      ref.invalidateSelf();
      await future;
    }();
  }
}

bool _sameOrigin(String relayUrl, String origin) {
  try {
    return normalizeRelayOrigin(relayUrl) == origin;
  } on FormatException {
    return false;
  }
}

bool _hasValidNsec(String? nsec) {
  if (nsec == null || nsec.isEmpty) return false;
  try {
    final decoded = nostr.Nip19.decode(payload: nsec);
    return decoded.prefix == nostr.Nip19Prefix.nsec &&
        decoded.data.length == 64;
  } catch (_) {
    return false;
  }
}

final authProvider = AsyncNotifierProvider<AuthNotifier, AuthState>(
  AuthNotifier.new,
);
