import 'dart:math';

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

enum SensitiveActionPolicy { enabled, disabledByUser }

class Community {
  final String id;
  final String name;
  final String relayUrl;
  final String? pubkey;
  final String? nsec;
  final SensitiveActionPolicy sensitiveActionPolicy;
  final bool pushNotificationsEnabled;
  final BuzzPushLeaseSubscriptionState pushSubscriptionState;

  /// Stable random address component for this community's relay push lease.
  ///
  /// Legacy records omit this value and continue using the endpoint grant's
  /// installation id so their already-published lease remains addressable.
  final String? pushLeaseInstallationId;

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

  /// Google account authorizing custody only; messaging still uses [nsec].
  final String? googleBackupAccountId;
  final DateTime addedAt;

  const Community({
    required this.id,
    required this.name,
    required this.relayUrl,
    this.pubkey,
    this.nsec,
    this.sensitiveActionPolicy = SensitiveActionPolicy.disabledByUser,
    this.pushNotificationsEnabled = false,
    this.pushSubscriptionState = const BuzzPushLeaseSubscriptionState.desired(),
    this.pushLeaseInstallationId,
    this.starterSetupIncomplete = false,
    this.tokenAuth = false,
    this.signedOut = false,
    this.googleBackupAccountId,
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
    BuzzPushLeaseSubscriptionState? pushSubscriptionState,
    Object? pushLeaseInstallationId = _sentinel,
    bool? starterSetupIncomplete,
    bool? tokenAuth,
    bool? signedOut,
    String? googleBackupAccountId,
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
      pushSubscriptionState:
          pushSubscriptionState ?? this.pushSubscriptionState,
      pushLeaseInstallationId: pushLeaseInstallationId == _sentinel
          ? this.pushLeaseInstallationId
          : pushLeaseInstallationId as String?,
      starterSetupIncomplete:
          starterSetupIncomplete ?? this.starterSetupIncomplete,
      tokenAuth: tokenAuth ?? this.tokenAuth,
      signedOut: signedOut ?? this.signedOut,
      googleBackupAccountId:
          googleBackupAccountId ?? this.googleBackupAccountId,
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
    'pushSubscriptionState': pushSubscriptionState.toJson(),
    if (pushLeaseInstallationId != null)
      'pushLeaseInstallationId': pushLeaseInstallationId,
    'starterSetupIncomplete': starterSetupIncomplete,
    if (tokenAuth) 'authMode': 'token',
    if (signedOut) 'signedOut': true,
    if (googleBackupAccountId != null)
      'googleBackupAccountId': googleBackupAccountId,
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
    final pushNotificationsEnabled =
        json['pushNotificationsEnabled'] as bool? ?? false;
    var pushSubscriptionState = json['pushSubscriptionState'] == null
        ? const BuzzPushLeaseSubscriptionState.desired()
        : BuzzPushLeaseSubscriptionState.fromJson(
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
      pushSubscriptionState: pushSubscriptionState,
      pushLeaseInstallationId: pushLeaseInstallationId,
      starterSetupIncomplete: json['starterSetupIncomplete'] as bool? ?? false,
      tokenAuth: json['authMode'] == 'token',
      signedOut: json['signedOut'] == true,
      googleBackupAccountId: json['googleBackupAccountId'] as String?,
      addedAt: DateTime.parse(json['addedAt'] as String),
    );
  }

  /// Gives the resumed session its own push lease, separate from old cleanup.
  Community resumeSession() => copyWith(
    signedOut: false,
    pushLeaseInstallationId: _newPushLeaseInstallationId(),
    pushSubscriptionState: BuzzPushLeaseSubscriptionState.desired(
      desired: pushSubscriptionState.desired,
    ),
  );

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
