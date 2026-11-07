import 'dart:math';

import 'package:nostr/nostr.dart' as nostr;
import 'package:uuid/uuid.dart';

import '../push/push_subscription.dart';

const _uuid = Uuid();
const _sentinel = Object();
final _pushLeaseInstallationIdPattern = RegExp(r'^[0-9a-f]{32}$');

String _newPushLeaseInstallationId() {
  final random = Random.secure();
  return List.generate(
    16,
    (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
  ).join();
}

String? _accountOf(String? nsec) {
  if (nsec == null || nsec.isEmpty) return null;
  try {
    final secret = nostr.Nip19.decode(payload: nsec).data;
    return secret.isEmpty ? null : nostr.Keys(secret).public;
  } catch (_) {
    return null;
  }
}

/// A fresh push inbox token: 16 random bytes as lowercase hex.
String newPushInboxToken() => _newPushLeaseInstallationId();

enum SensitiveActionPolicy { enabled, disabledByUser }

/// The person's own push choice. Absent until they flip the switch, so push
/// can start on by default without overriding an explicit off.
enum KuzzPushChoice { on, off }

class Community {
  final String id;
  final String name;
  final String relayUrl;
  final String? pubkey;
  final String? nsec;
  final SensitiveActionPolicy sensitiveActionPolicy;
  final bool pushNotificationsEnabled;

  /// Set only when the person turned push on or off themselves, or when the
  /// one-time default-on migration turned it on. Null means never chosen.
  final KuzzPushChoice? pushChoice;
  final KuzzPushLeaseSubscriptionState pushSubscriptionState;

  /// Stable random address component for this community's relay push lease.
  ///
  /// Legacy records omit this value and continue using the endpoint grant's
  /// installation id so their already-published lease remains addressable.
  final String? pushLeaseInstallationId;

  /// Names this (community, account)'s push inbox folder, where the iPhone
  /// notification extension keeps the events it fetched
  /// (docs/message-sync.md#push-inbox). Random and persisted; created when
  /// push turns on, replaced on sign-out, resume and account change.
  final String? pushInboxToken;

  /// Whether invite-created starter channels still need to be recovered.
  final bool starterSetupIncomplete;

  /// Whether this community signs in with a relay token session
  /// (centralized identity, plan §3.2) instead of a local `nsec`.
  ///
  /// Token communities authenticate with the origin-scoped refresh token in
  /// `RefreshTokenStore`; [pubkey] holds the principal id and [nsec] is
  /// normally absent (a legacy key, if any, is kept untouched).
  final bool tokenAuth;

  /// Local session ended. Membership and secure recovery material are retained,
  /// but only explicit, identity-verified sign-in may resume this community.
  final bool signedOut;

  /// A sign-out of this account has begun and not finished. Written before
  /// the session is ended, so a sign-out cut short (the app killed halfway)
  /// is finished on the next launch instead of being taken for a lost
  /// session. Cleared by the sign-out itself and by any sign-in.
  final bool signingOut;

  /// Google account authorizing custody only; messaging still uses [nsec].
  final String? googleBackupAccountId;

  /// How this account last signed in on this phone (`google` or `apple`),
  /// recorded locally at sign-in. Absent on older records, which were all
  /// Google: read it through [signInMethod].
  final String? signInProvider;

  /// When this account last signed in or was switched to on this phone.
  /// Absent on older records. Read by [mostRecentlyUsedCommunity].
  final DateTime? lastUsedAt;

  /// This account's name, photo and email as last loaded, so the account list
  /// can draw every account from this phone. Absent until the account's
  /// profile first loads while it is active.
  final String? displayName;
  final String? avatarUrl;
  final String? email;
  final DateTime addedAt;

  const Community({
    required this.id,
    required this.name,
    required this.relayUrl,
    this.pubkey,
    this.nsec,
    this.sensitiveActionPolicy = SensitiveActionPolicy.disabledByUser,
    this.pushNotificationsEnabled = false,
    this.pushChoice,
    this.pushSubscriptionState = const KuzzPushLeaseSubscriptionState.desired(),
    this.pushLeaseInstallationId,
    this.pushInboxToken,
    this.starterSetupIncomplete = false,
    this.tokenAuth = false,
    this.signedOut = false,
    this.signingOut = false,
    this.googleBackupAccountId,
    this.signInProvider,
    this.lastUsedAt,
    this.displayName,
    this.avatarUrl,
    this.email,
    required this.addedAt,
  });

  factory Community.create({
    required String name,
    required String relayUrl,
    String? pubkey,
    String? nsec,
    SensitiveActionPolicy sensitiveActionPolicy =
        SensitiveActionPolicy.disabledByUser,
    bool starterSetupIncomplete = false,
    bool tokenAuth = false,
    String? googleBackupAccountId,
    String? signInProvider,
  }) {
    return Community(
      id: _uuid.v4(),
      name: name,
      relayUrl: relayUrl,
      pubkey: pubkey,
      nsec: nsec,
      sensitiveActionPolicy: sensitiveActionPolicy,
      pushLeaseInstallationId: _newPushLeaseInstallationId(),
      starterSetupIncomplete: starterSetupIncomplete,
      tokenAuth: tokenAuth,
      googleBackupAccountId: googleBackupAccountId,
      signInProvider: signInProvider,
      addedAt: DateTime.now(),
    );
  }

  Community copyWith({
    String? name,
    String? relayUrl,
    Object? pubkey = _sentinel,
    Object? nsec = _sentinel,
    SensitiveActionPolicy? sensitiveActionPolicy,
    bool? pushNotificationsEnabled,
    KuzzPushChoice? pushChoice,
    KuzzPushLeaseSubscriptionState? pushSubscriptionState,
    Object? pushLeaseInstallationId = _sentinel,
    Object? pushInboxToken = _sentinel,
    bool? starterSetupIncomplete,
    bool? tokenAuth,
    bool? signedOut,
    bool? signingOut,
    String? googleBackupAccountId,
    String? signInProvider,
    DateTime? lastUsedAt,
    Object? displayName = _sentinel,
    Object? avatarUrl = _sentinel,
    Object? email = _sentinel,
  }) {
    return Community(
      id: id,
      name: name ?? this.name,
      relayUrl: relayUrl ?? this.relayUrl,
      pubkey: pubkey == _sentinel ? this.pubkey : pubkey as String?,
      nsec: nsec == _sentinel ? this.nsec : nsec as String?,
      sensitiveActionPolicy:
          sensitiveActionPolicy ?? this.sensitiveActionPolicy,
      pushNotificationsEnabled:
          pushNotificationsEnabled ?? this.pushNotificationsEnabled,
      pushChoice: pushChoice ?? this.pushChoice,
      pushSubscriptionState:
          pushSubscriptionState ?? this.pushSubscriptionState,
      pushLeaseInstallationId: pushLeaseInstallationId == _sentinel
          ? this.pushLeaseInstallationId
          : pushLeaseInstallationId as String?,
      pushInboxToken: pushInboxToken == _sentinel
          ? this.pushInboxToken
          : pushInboxToken as String?,
      starterSetupIncomplete:
          starterSetupIncomplete ?? this.starterSetupIncomplete,
      tokenAuth: tokenAuth ?? this.tokenAuth,
      signedOut: signedOut ?? this.signedOut,
      signingOut: signingOut ?? this.signingOut,
      googleBackupAccountId:
          googleBackupAccountId ?? this.googleBackupAccountId,
      signInProvider: signInProvider ?? this.signInProvider,
      lastUsedAt: lastUsedAt ?? this.lastUsedAt,
      displayName: displayName == _sentinel
          ? this.displayName
          : displayName as String?,
      avatarUrl: avatarUrl == _sentinel ? this.avatarUrl : avatarUrl as String?,
      email: email == _sentinel ? this.email : email as String?,
      addedAt: addedAt,
    );
  }

  Map<String, dynamic> toJson() => {
    'id': id,
    'name': name,
    'relayUrl': relayUrl,
    if (pubkey != null) 'pubkey': pubkey,
    if (nsec != null) 'nsec': nsec,
    'sensitiveActionPolicy': sensitiveActionPolicy.name,
    'pushNotificationsEnabled': pushNotificationsEnabled,
    if (pushChoice != null) 'pushChoice': pushChoice!.name,
    'pushSubscriptionState': pushSubscriptionState.toJson(),
    if (pushLeaseInstallationId != null)
      'pushLeaseInstallationId': pushLeaseInstallationId,
    if (pushInboxToken != null) 'pushInboxToken': pushInboxToken,
    'starterSetupIncomplete': starterSetupIncomplete,
    if (tokenAuth) 'authMode': 'token',
    if (signedOut) 'signedOut': true,
    if (signingOut) 'signingOut': true,
    if (googleBackupAccountId != null)
      'googleBackupAccountId': googleBackupAccountId,
    if (signInProvider != null) 'signInProvider': signInProvider,
    if (lastUsedAt != null) 'lastUsedAt': lastUsedAt!.toIso8601String(),
    if (displayName != null) 'displayName': displayName,
    if (avatarUrl != null) 'avatarUrl': avatarUrl,
    if (email != null) 'email': email,
    'addedAt': addedAt.toIso8601String(),
  };

  factory Community.fromJson(Map<String, dynamic> json) {
    final pushLeaseInstallationId = json['pushLeaseInstallationId'] as String?;
    if (pushLeaseInstallationId != null &&
        !_pushLeaseInstallationIdPattern.hasMatch(pushLeaseInstallationId)) {
      throw const FormatException(
        'Push lease installation id must be 16 random bytes encoded as lowercase hex',
      );
    }
    final pushInboxToken = json['pushInboxToken'] as String?;
    if (pushInboxToken != null &&
        !_pushLeaseInstallationIdPattern.hasMatch(pushInboxToken)) {
      throw const FormatException(
        'Push inbox token must be 16 random bytes encoded as lowercase hex',
      );
    }
    final pushNotificationsEnabled =
        json['pushNotificationsEnabled'] as bool? ?? false;
    var pushSubscriptionState = json['pushSubscriptionState'] == null
        ? const KuzzPushLeaseSubscriptionState.desired()
        : KuzzPushLeaseSubscriptionState.fromJson(
            Map<String, dynamic>.from(json['pushSubscriptionState'] as Map),
          );
    if (!pushNotificationsEnabled &&
        pushSubscriptionState.pendingTombstoneGeneration == null) {
      pushSubscriptionState = pushSubscriptionState
          .withPendingTombstoneAtCursor();
    }
    return Community(
      id: json['id'] as String,
      name: json['name'] as String,
      relayUrl: json['relayUrl'] as String,
      pubkey: json['pubkey'] as String?,
      nsec: json['nsec'] as String?,
      sensitiveActionPolicy: SensitiveActionPolicy.values.firstWhere(
        (value) => value.name == json['sensitiveActionPolicy'],
        orElse: () => SensitiveActionPolicy.disabledByUser,
      ),
      pushNotificationsEnabled: pushNotificationsEnabled,
      pushChoice: KuzzPushChoice.values
          .where((value) => value.name == json['pushChoice'])
          .firstOrNull,
      pushSubscriptionState: pushSubscriptionState,
      pushLeaseInstallationId: pushLeaseInstallationId,
      pushInboxToken: pushInboxToken,
      starterSetupIncomplete: json['starterSetupIncomplete'] as bool? ?? false,
      tokenAuth: json['authMode'] == 'token',
      signedOut: json['signedOut'] == true,
      signingOut: json['signingOut'] == true,
      googleBackupAccountId: json['googleBackupAccountId'] as String?,
      signInProvider: json['signInProvider'] as String?,
      lastUsedAt: DateTime.tryParse(json['lastUsedAt'] as String? ?? ''),
      displayName: json['displayName'] as String?,
      avatarUrl: json['avatarUrl'] as String?,
      email: json['email'] as String?,
      addedAt: DateTime.parse(json['addedAt'] as String),
    );
  }

  /// The public key of this account's signing identity.
  String? get accountPubkey => pubkey ?? _accountOf(nsec);

  /// The sign-in method to show: [signInProvider], or `google` for records
  /// made before it was recorded.
  String get signInMethod => signInProvider ?? 'google';

  /// Gives the resumed session its own push lease, separate from old cleanup.
  Community resumeSession() => copyWith(
    signedOut: false,
    pushLeaseInstallationId: _newPushLeaseInstallationId(),
    pushInboxToken: newPushInboxToken(),
    pushSubscriptionState: KuzzPushLeaseSubscriptionState.desired(
      desired: pushSubscriptionState.desired,
    ),
  );

  /// This record with a push inbox token, if push is on, the session is
  /// live and it has none yet. Otherwise unchanged.
  Community withPushInboxToken() =>
      pushNotificationsEnabled && !signedOut && pushInboxToken == null
      ? copyWith(pushInboxToken: newPushInboxToken())
      : this;

  /// This record with push on, unless the person turned it off themselves
  /// or this account can't use push (token sign-in, no key, signed out).
  /// Gets a push inbox token the same way the switch does.
  Community withPushOnByDefault() =>
      pushNotificationsEnabled ||
          pushChoice == KuzzPushChoice.off ||
          signedOut ||
          tokenAuth ||
          nsec == null ||
          nsec!.isEmpty
      ? this
      : copyWith(pushNotificationsEnabled: true).withPushInboxToken();

  /// Credentials [pubkey]/[nsec] for this community. A different account
  /// gets a new push inbox token so it never reads the old one's folder.
  Community withCredentials({String? pubkey, String? nsec}) {
    final before = (this.pubkey ?? _accountOf(this.nsec))?.toLowerCase();
    final after = (pubkey ?? _accountOf(nsec))?.toLowerCase();
    return copyWith(
      pubkey: pubkey,
      nsec: nsec,
      pushInboxToken: before == after || pushInboxToken == null
          ? pushInboxToken
          : newPushInboxToken(),
    );
  }

  /// This record with [name] and [avatar] as the cached profile. A blank name
  /// keeps the cached one. A photo stored inline can be large, and this
  /// record is read whole everywhere, so one over [maxCachedAvatarLength] is
  /// not kept.
  Community withCachedProfile({
    required String? name,
    required String? avatar,
  }) {
    final trimmed = name?.trim();
    return copyWith(
      displayName: trimmed == null || trimmed.isEmpty ? displayName : trimmed,
      avatarUrl: avatar != null && avatar.length <= maxCachedAvatarLength
          ? avatar
          : null,
    );
  }

  /// Longest photo address kept in the record.
  static const maxCachedAvatarLength = 2048;

  /// Derive a human-friendly community name from a relay URL.
  static String nameFromUrl(String url) {
    try {
      final host = Uri.parse(url).host;
      if (host.contains('localhost') || host == '127.0.0.1') return 'Local Dev';
      final parts = host.split('.');
      if (parts.length > 2) return parts.first;
      return host;
    } catch (_) {
      return 'Community';
    }
  }
}
