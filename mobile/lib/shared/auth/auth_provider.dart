import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../features/channels/channel_head_cache.dart';
import '../community/community.dart';
import '../community/community_accounts.dart';
import '../community/community_provider.dart';
import '../community/community_token_session.dart';
import '../message_store/message_store_provider.dart';
import '../push/push_bridge.dart';
import '../push/push_inbox.dart';
import '../push/push_subscription.dart';
import '../relay/relay_provider.dart';
import 'default_community.dart';
import 'google_key_backup.dart';
import 'token/relay_origin.dart';
import 'token/session_key.dart';
import 'token/token_session.dart';
import 'token/token_session_provider.dart';
import '../l10n/l10n.dart';

enum AuthStatus { unknown, unauthenticated, authenticated }

class AuthState {
  final AuthStatus status;
  final Community? community;
  final String? logoutError;

  const AuthState({required this.status, this.community, this.logoutError});
}

/// Restores the active community without making connectivity load-bearing.
/// The relay session owns connection recovery after startup.
class AuthNotifier extends AsyncNotifier<AuthState> {
  /// Sign-outs started per community: stops a sign-in of that account still
  /// running, and only that one.
  final Map<String, int> _signOutGenerations = {};

  int _signOutGeneration(String communityId) =>
      _signOutGenerations[communityId] ?? 0;

  void _noteSignOut(String communityId) =>
      _signOutGenerations[communityId] = _signOutGeneration(communityId) + 1;

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

    // Startup pinning: with a built-in server only its entries can be
    // selected; entries for other servers stay stored but unused.
    final builtInOrigin = ref.read(defaultCommunityOriginProvider);
    var activeId = await storage.loadActiveId();
    while (true) {
      final active = selectActiveCommunity(
        communities,
        activeId,
        builtInOrigin: builtInOrigin,
      );
      if (active == null) break;
      await storage.saveActiveId(active.id);

      if (active.signedOut) {
        return _signedOutState(active);
      }

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
  ///
  /// With [attempt] the community's account takes over that sign-in's
  /// session here too: row and session commit together, inside one exclusive
  /// transition, row first, so there is never a stored token without a row.
  /// If the session cannot be taken over the row is put back and this throws.
  /// The attempt stays the caller's to discard.
  ///
  /// A signed-out row comes back only from a signed-out copy, and a live row
  /// only from a live one: anything else was read before a sign-out or
  /// another sign-in finished, and is cancelled. The same account with
  /// another key (its row's key is not the one the server pairs with it)
  /// leaves the old key's lease, messages and heads behind.
  Future<void> authenticateWithCommunity(
    Community community, {
    bool Function()? isCurrent,
    TokenSessionController? attempt,
  }) {
    final generation = _signOutGeneration(community.id);
    return ref.read(communityTransitionProvider).runExclusive(() async {
      void guard() {
        if (generation != _signOutGeneration(community.id) ||
            isCurrent?.call() == false) {
          throw StateError('Sign-in cancelled');
        }
      }

      guard();
      if (!isServerAllowed(
        community.relayUrl,
        builtInOrigin: ref.read(defaultCommunityOriginProvider),
      )) {
        throw StateError(differentServerMessage);
      }
      final storage = ref.read(communityStorageProvider);
      final existing = (await storage.loadAll())
          .where((saved) => saved.id == community.id)
          .firstOrNull;
      if ((existing?.signedOut ?? false) != community.signedOut) {
        throw StateError('Sign-in cancelled');
      }
      final keyChanged =
          existing != null &&
          existing.accountPubkey != null &&
          existing.accountPubkey != community.accountPubkey;
      if (community.signedOut || keyChanged) {
        community = community.resumeSession();
      }
      if (ref.read(kuzzPushBuildSupportedProvider)) {
        // Like other messengers, notifications start on at sign-in; the
        // system prompt follows on the first Home screen.
        community = community.withPushOnByDefault();
      }
      community = community.copyWith(lastUsedAt: DateTime.now());
      final previousActiveId = await storage.loadActiveId();
      // The same account signing in again leaves the screen it is on, so a
      // huddle or read-aloud of that account keeps going.
      final sameAccount =
          existing != null &&
          !existing.signedOut &&
          !keyChanged &&
          previousActiveId == community.id &&
          SessionKey.ofCommunity(existing) == SessionKey.ofCommunity(community);
      if (!sameAccount) await ref.read(communityTransitionProvider).run();
      guard();
      final revocationJournaled = keyChanged && !existing.signedOut
          ? await ref.read(communityPushLeaseRevocationEnqueuerProvider)(
              existing,
            )
          : false;
      await storage.save(community);
      await storage.saveActiveId(community.id);
      if (attempt != null) {
        try {
          await _adopt(community, attempt);
        } catch (_) {
          if (existing != null) {
            await storage.save(existing);
          } else {
            await storage.remove(community.id);
          }
          if (previousActiveId != null) {
            await storage.saveActiveId(previousActiveId);
          } else {
            await storage.clearActiveId();
          }
          rethrow;
        }
      }
      await syncStoredCommunitySnapshot(ref);

      // Invalidate community providers so other consumers pick up the new data.
      ref.invalidate(communityListProvider);
      ref.invalidate(activeCommunityProvider);

      state = AsyncData(
        AuthState(status: AuthStatus.authenticated, community: community),
      );
      await _dropPendingKey(community);
      if (keyChanged) {
        _forgetAccountData(existing);
        unawaited(
          deletePushInboxFolders(ref.read(pushInboxRootProvider), [
            existing.pushInboxToken,
          ]),
        );
      }
      if (revocationJournaled) _triggerPushLeaseRevocation();
    });
  }

  /// [community]'s row holds its key now. A leftover is only a stray copy, so
  /// failing to delete it fails nothing.
  Future<void> _dropPendingKey(Community community) async {
    final account = community.googleBackupAccountId;
    if (account == null) return;
    try {
      await ref
          .read(pendingBackupKeyStoreProvider)
          .delete(community.relayUrl, account);
    } catch (error) {
      debugPrint(
        '[AuthNotifier] pending key not deleted: ${error.runtimeType}',
      );
    }
  }

  /// Moves [attempt]'s session to the account of [community].
  Future<void> _adopt(
    Community community,
    TokenSessionController attempt,
  ) async {
    final key = SessionKey.ofCommunity(community);
    if (key == null) throw StateError('Sign-in has no account to go to');
    final handoff = await attempt.handoff();
    await ref.read(sessionControllerProviderOf(community, key)).adopt(handoff);
  }

  /// Record a completed token sign-in for [relayUrl] and switch to it.
  ///
  /// Reuses the row of [principalId] (marking it as token-authenticated),
  /// otherwise creates one. See [authenticateWithCommunity] for [attempt].
  ///
  /// A reused legacy community's push lease was published by its `nsec`, so
  /// its revocation is durably journaled with that key first (AGENTS.md
  /// rule 1; a journal failure propagates and changes nothing). Only then is
  /// the community rewritten once, without the key and with push off
  /// (rule 5), so no state pairs the principal with a key it does not own.
  Future<void> authenticateWithTokenSession({
    required String relayUrl,
    required String principalId,
    bool Function()? isCurrent,
    TokenSessionController? attempt,
  }) async {
    final origin = normalizeRelayOrigin(relayUrl);
    if (!isServerAllowed(
      origin,
      builtInOrigin: ref.read(defaultCommunityOriginProvider),
    )) {
      throw StateError(differentServerMessage);
    }
    final existing = findTokenAccountRow(
      await ref.read(communityStorageProvider).loadAll(),
      origin: origin,
      principal: principalId,
    );
    final legacyKey = existing?.nsec;
    var revocationJournaled = false;
    if (existing != null && legacyKey != null && legacyKey.isNotEmpty) {
      revocationJournaled = await ref.read(
        communityPushLeaseRevocationEnqueuerProvider,
      )(existing);
    }
    final community = existing != null
        ? existing
              .withCredentials(
                pubkey: principalId,
                nsec: legacyKey == null ? existing.nsec : null,
              )
              .copyWith(
                tokenAuth: true,
                pushNotificationsEnabled: legacyKey == null
                    ? existing.pushNotificationsEnabled
                    : false,
                pushSubscriptionState: legacyKey == null
                    ? existing.pushSubscriptionState
                    : const KuzzPushLeaseSubscriptionState.desired(),
              )
        : Community.create(
            name: Community.nameFromUrl(origin),
            relayUrl: origin,
            pubkey: principalId,
            tokenAuth: true,
          );
    await authenticateWithCommunity(
      community,
      isCurrent: isCurrent,
      attempt: attempt,
    );
    if (revocationJournaled) _triggerPushLeaseRevocation();
  }

  /// Remove the community on screen when this is called, even if another
  /// becomes active before it runs. A token community's device session is
  /// revoked on the relay first; when the relay cannot be reached this throws
  /// `TokenSessionException` and removes nothing. [deviceOnly] skips the
  /// relay (see `CommunityListNotifier.removeCommunity`).
  Future<void> removeActiveCommunity({bool deviceOnly = false}) {
    final shown = state.value?.community?.id;
    return () async {
      final storage = ref.read(communityStorageProvider);
      await ref
          .read(communityListProvider.notifier)
          .removeActiveCommunityForSignOut(id: shown, deviceOnly: deviceOnly);

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

  /// Ends the session of the account on screen when this is called, even if
  /// another becomes active before it runs; every other account stays signed
  /// in and the most recently used one becomes active if this one was. The
  /// account stays listed. A person key backed up to Google is deleted from
  /// this phone; a key with no backup is kept, since nothing else could
  /// restore it.
  Future<void> signOut({bool deviceOnly = false}) {
    // At once: a sign-in of this account must not finish behind the sign-out.
    final shown = state.value?.community?.id;
    if (shown != null) _noteSignOut(shown);
    return ref.read(communityTransitionProvider).runExclusive(() async {
      final storage = ref.read(communityStorageProvider);
      final communities = await storage.loadAll();
      final target = shown == null
          ? selectActiveCommunity(
              communities,
              await storage.loadActiveId(),
              builtInOrigin: ref.read(defaultCommunityOriginProvider),
            )
          : communities.where((row) => row.id == shown).firstOrNull;
      if (target == null) return;
      if (target.id != shown) _noteSignOut(target.id);
      await _signOutAccount(target, communities, deviceOnly: deviceOnly);
    });
  }

  /// Ends the session of one community only (the relay signed it out from
  /// another device, or it has no session left); every other account stays
  /// signed in. When it is the active community, this goes on as [signOut]
  /// does.
  ///
  /// With [isCurrent] the reason is about a session that a sign-in may
  /// replace while this waits its turn. It is asked when the turn comes, and
  /// without it nothing happens: the row and the session in it are newer.
  Future<void> signOutCommunity(
    String communityId, {
    bool deviceOnly = false,
    bool Function()? isCurrent,
  }) {
    if (isCurrent == null) _noteSignOut(communityId);
    return ref.read(communityTransitionProvider).runExclusive(() async {
      if (isCurrent?.call() == false) return;
      final communities = await ref.read(communityStorageProvider).loadAll();
      final target = communities
          .where((community) => community.id == communityId)
          .firstOrNull;
      if (target == null) return;
      if (isCurrent != null) {
        if (target.signedOut) return;
        _noteSignOut(communityId);
      }
      await _signOutAccount(target, communities, deviceOnly: deviceOnly);
    });
  }

  Future<void> _signOutAccount(
    Community target,
    List<Community> communities, {
    required bool deviceOnly,
  }) async {
    final storage = ref.read(communityStorageProvider);
    final builtInOrigin = ref.read(defaultCommunityOriginProvider);
    final wasActive =
        selectActiveCommunity(
          communities,
          await storage.loadActiveId(),
          builtInOrigin: builtInOrigin,
        )?.id ==
        target.id;
    var hasRevocations = false;
    if (!target.signedOut) {
      // First: a failure here leaves the account as it was, with nothing
      // queued against a session that is still going.
      await ref.read(communityLogoutSessionEnderProvider)(
        target,
        deviceOnly: deviceOnly,
      );
      hasRevocations = await ref.read(
        communityPushLeaseRevocationEnqueuerProvider,
      )(target);
    }
    // Another account's huddle or read-aloud has nothing to do with this one.
    if (wasActive) await ref.read(communityTransitionProvider).run();
    final signedOut = [
      for (final community in communities)
        community.id == target.id ? _signedOutCopy(community) : community,
    ];
    // One durable commit: the session stops; the identity stays listed.
    // A person key held by Google is deleted from this phone (signing in
    // with Google restores it); this install's device key is kept.
    await storage.saveAll(signedOut);
    // Messages and cached heads kept on this phone go with the account.
    _forgetAccountData(target);
    // So does the push inbox, once the notification extension no longer
    // writes for it (the launch sweep removes what is left).
    final inboxToken = target.pushInboxToken;
    if (inboxToken != null) {
      await syncCommunitySnapshot(ref, signedOut);
      unawaited(
        deletePushInboxFolders(ref.read(pushInboxRootProvider), [inboxToken]),
      );
    }
    final next = wasActive
        ? selectActiveCommunity(signedOut, null, builtInOrigin: builtInOrigin)
        : null;
    if (!wasActive || (next != null && !next.signedOut)) {
      if (next != null) await storage.saveActiveId(next.id);
      await syncCommunitySnapshot(ref, signedOut);
      ref.invalidate(communityListProvider);
      if (next != null) {
        ref.invalidate(activeCommunityProvider);
        // The start-up path: [next] may not be usable as it stands.
        ref.invalidateSelf();
        await future;
      }
      if (hasRevocations) _triggerPushLeaseRevocation();
      return;
    }
    final active = signedOut.firstWhere((item) => item.id == target.id);
    state = AsyncData(
      AuthState(
        status: AuthStatus.unauthenticated,
        community: active.copyWith(nsec: null, pushNotificationsEnabled: false),
        logoutError: appL10n.authSignOutFinishing,
      ),
    );
    state = AsyncData(await _signedOutState(active));
    ref.invalidate(communityListProvider);
    ref.invalidate(activeCommunityProvider);
    if (hasRevocations) _triggerPushLeaseRevocation();
  }

  /// Drops what this phone keeps for [community]'s account, and nothing of
  /// any other account.
  void _forgetAccountData(Community community) {
    final account = community.accountPubkey;
    if (account == null) return;
    unawaited(
      ref.read(messageStoreProvider).forget(community.relayUrl, account),
    );
    unawaited(
      ref
          .read(channelHeadCacheProvider)
          .clearAccount(
            ChannelHeadScope(
              relayUrl: RelayConfig(baseUrl: community.relayUrl).baseUrl,
              pubkey: account,
            ),
          )
          .catchError((Object error) {
            debugPrint('[AuthNotifier] failed to clear channel heads: $error');
          }),
    );
  }

  void _triggerPushLeaseRevocation() => unawaited(
    ref
        .read(communityPushLeaseRevocationTriggerProvider)()
        .catchError(reportPushLeaseCleanupError),
  );

  Future<AuthState> _signedOutState(Community? active) async {
    String? error;
    try {
      await settleSignedOutCommunitySnapshot(ref);
    } catch (_) {
      error = appL10n.authNotificationsOffFailed;
    }
    return AuthState(
      status: AuthStatus.unauthenticated,
      community: active?.copyWith(nsec: null, pushNotificationsEnabled: false),
      logoutError: error,
    );
  }

  /// Explicit key proof for resuming a signed-out key account. No screen calls
  /// this: users only come back through Google. Kept as the tested seam for the
  /// resume fence and push-lease renewal (non_destructive_sign_out_test.dart).
  @visibleForTesting
  Future<void> signInWithPrivateKey(String nsec) async {
    final storage = ref.read(communityStorageProvider);
    final activeId = await storage.loadActiveId();
    final existing = (await storage.loadAll())
        .where((community) => community.id == activeId)
        .firstOrNull;
    final supplied = _pubkey(nsec.trim());
    if (existing == null ||
        !existing.signedOut ||
        supplied == null ||
        supplied != existing.accountPubkey) {
      throw StateError(
        'That doesn’t match this account. Check it and try again.',
      );
    }
    await authenticateWithCommunity(existing.copyWith(nsec: nsec.trim()));
  }
}

/// [community] signed out. A person key backed up to Google is deleted
/// (its public key is kept, so the next Google sign-in restores the same
/// account); one with no backup is kept.
Community _signedOutCopy(Community community) {
  if (community.googleBackupAccountId == null) {
    return community.copyWith(signedOut: true, pushInboxToken: null);
  }
  return community.copyWith(
    signedOut: true,
    pubkey: community.accountPubkey,
    nsec: null,
    pushInboxToken: null,
  );
}

String? _pubkey(String? nsec) {
  if (!_hasValidNsec(nsec)) return null;
  return nostr.Keys(nostr.Nip19.decode(payload: nsec!).data).public;
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
