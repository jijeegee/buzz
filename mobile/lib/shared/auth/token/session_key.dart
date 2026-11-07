import 'package:flutter/foundation.dart';

import '../../community/community.dart';
import 'relay_origin.dart';

/// Names one account's session: a canonical relay origin and the principal
/// signed in there. Several accounts can share one origin, so an origin alone
/// never identifies a session.
///
/// The principal is the `googleBackupAccountId` of a key community and the
/// `pubkey` of a token community. Throws [FormatException] when `origin` is
/// not a relay URL (see [normalizeRelayOrigin]).
@immutable
class SessionKey {
  SessionKey(String origin, this.principal)
    : origin = normalizeRelayOrigin(origin);

  /// The session of [community], or `null` when it has none: an older
  /// key-only community, or an address that is not a relay URL.
  static SessionKey? ofCommunity(Community community) {
    final principal = community.tokenAuth
        ? community.pubkey
        : community.googleBackupAccountId;
    if (principal == null || principal.isEmpty) return null;
    try {
      return SessionKey(community.relayUrl, principal);
    } on FormatException {
      return null;
    }
  }

  final String origin;
  final String principal;

  @override
  bool operator ==(Object other) =>
      other is SessionKey &&
      other.origin == origin &&
      other.principal == principal;

  @override
  int get hashCode => Object.hash(origin, principal);

  @override
  String toString() => 'SessionKey($origin, $principal)';
}
