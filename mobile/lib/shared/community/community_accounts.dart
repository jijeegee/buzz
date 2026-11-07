import '../auth/default_community.dart';
import '../auth/token/relay_origin.dart';
import '../relay/service_address.dart';
import 'community.dart';

/// Rows of one service: every address it has had counts as the same service.
Iterable<Community> communitiesOfService(
  Iterable<Community> communities,
  String origin,
) {
  final scope = storageScopeFor(origin);
  return communities.where((community) {
    try {
      return storageScopeFor(normalizeRelayOrigin(community.relayUrl)) == scope;
    } on FormatException {
      return false;
    }
  });
}

/// The row of a Google or Apple [account], or `null` when it has none: that
/// account gets a new row, it never takes over another one.
///
/// [pubkey] is the key the server pairs with [account]. It also finds an older
/// key-only row holding exactly that key, which records no account yet.
Community? findKeyAccountRow(
  Iterable<Community> communities, {
  required String origin,
  required String account,
  String? pubkey,
}) {
  final rows = communitiesOfService(communities, origin).toList();
  return rows
          .where((community) => community.googleBackupAccountId == account)
          .firstOrNull ??
      rows
          .where(
            (community) =>
                pubkey != null &&
                community.pubkey == pubkey &&
                !community.tokenAuth &&
                community.googleBackupAccountId == null,
          )
          .firstOrNull;
}

/// The row of the token account [principal], or of the older key-only account
/// with that public key. `null`: that account gets a new row.
Community? findTokenAccountRow(
  Iterable<Community> communities, {
  required String origin,
  required String principal,
}) => communitiesOfService(communities, origin)
    .where(
      (community) =>
          community.googleBackupAccountId == null &&
          community.accountPubkey == principal,
    )
    .firstOrNull;

/// The accounts signed in on this phone, most recently used first. Rows never
/// used since that was recorded come last, in stored order.
List<Community> signedInAccountsByRecent(
  Iterable<Community> communities, {
  required String? builtInOrigin,
}) {
  final rows = [
    for (final community in communities)
      if (!community.signedOut &&
          isServerAllowed(community.relayUrl, builtInOrigin: builtInOrigin))
        community,
  ];
  final order = {for (final (i, row) in rows.indexed) row.id: i};
  return rows..sort((a, b) {
    final aUsed = a.lastUsedAt;
    final bUsed = b.lastUsedAt;
    if (aUsed == null || bUsed == null) {
      if (aUsed != null) return -1;
      if (bUsed != null) return 1;
    } else if (aUsed != bUsed) {
      return bUsed.compareTo(aUsed);
    }
    return order[a.id]!.compareTo(order[b.id]!);
  });
}

/// The signed-in row used most recently. Rows never used since this was
/// recorded come last, in stored order. `null` when none is signed in.
Community? mostRecentlyUsedCommunity(Iterable<Community> communities) {
  Community? best;
  for (final community in communities) {
    if (community.signedOut) continue;
    final used = community.lastUsedAt;
    final bestUsed = best?.lastUsedAt;
    if (best == null ||
        (used != null && (bestUsed == null || used.isAfter(bestUsed)))) {
      best = community;
    }
  }
  return best;
}
