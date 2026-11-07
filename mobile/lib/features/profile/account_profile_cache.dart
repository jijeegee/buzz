import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/auth/token/token.dart';
import '../../shared/community/community.dart';
import '../../shared/community/community_provider.dart';
import '../../shared/relay/relay.dart';
import 'profile_provider.dart';

/// Keeps the active account's name, photo and email on its row, so the account
/// list can draw every account from this phone.
///
/// A token account has one profile, from its account. A key account publishes
/// its own name and photo; the account adds the email, if it has an account.
final accountProfileCacheProvider = Provider<void>((ref) {
  final activeId = ref.watch(
    activeCommunityProvider.select((active) => active.value?.id),
  );
  final tokenAuth = ref.watch(
    activeCommunityProvider.select((active) => active.value?.tokenAuth),
  );
  final pubkey = ref.watch(myPubkeyProvider)?.toLowerCase();
  final session = ref.watch(accountSessionProvider);
  if (activeId == null || tokenAuth == null || pubkey == null) return;

  void cache(Community Function(Community row) update) {
    final rows = ref.read(communityListProvider.notifier);
    unawaited(
      rows
          .cacheProfile(activeId, update)
          .catchError(
            (Object error) => debugPrint('Profile cache failed: $error'),
          ),
    );
  }

  if (session != null) {
    final signedIn = ref.watch(
      (tokenAuth
              ? tokenSessionProvider(session)
              : keyBackupSessionProvider(session))
          .select((state) => state.status == TokenSessionStatus.signedIn),
    );
    if (signedIn) {
      ref.listen(accountProfileProvider(session), (_, next) {
        if (next.isLoading) return;
        final profile = next.asData?.value;
        if (profile == null) return;
        cache(
          (row) => tokenAuth
              ? row
                    .withCachedProfile(
                      name: profile.displayName,
                      avatar: profile.avatarUrl,
                    )
                    .copyWith(email: profile.email)
              : row.copyWith(email: profile.email),
        );
      }, fireImmediately: true);
    }
  }
  if (!tokenAuth) {
    ref.listen(profileProvider, (_, next) {
      final profile = next.value;
      if (next.isLoading || profile == null || profile.pubkey != pubkey) return;
      cache(
        (row) => row.withCachedProfile(
          name: profile.displayName,
          avatar: profile.avatarUrl,
        ),
      );
    }, fireImmediately: true);
  }
});
