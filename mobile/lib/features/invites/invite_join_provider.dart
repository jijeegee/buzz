import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';

import 'package:http/http.dart' as http;
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../shared/auth/auth.dart';
import '../../shared/auth/default_community.dart';
import '../../shared/auth/token/relay_origin.dart';
import '../../shared/auth/token/session_key.dart';
import '../../shared/auth/token/token_auth_descriptor.dart';
import '../../shared/auth/token/token_session_provider.dart';
import '../../shared/deeplink/deep_link.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/relay/relay_access_tokens.dart';
import '../../shared/relay/relay_provider.dart';
import '../../shared/relay/relay_session.dart';
import '../../shared/relay/relay_validation.dart';

final inviteJoinHttpClientProvider = Provider<http.Client>((ref) {
  final client = http.Client();
  ref.onDispose(client.close);
  return client;
});

final inviteKeyGeneratorProvider = Provider<InviteKeyGenerator>((ref) {
  return () => nostr.Keys.generate();
});

typedef InviteKeyGenerator = nostr.Keys Function();

/// Bearer source for claiming an invite on a token-auth community, keyed by
/// its account (the community need not be active yet).
final inviteClaimAccessTokensProvider =
    Provider.family<RelayAccessTokens, SessionKey>(
      (ref, key) => ControllerRelayAccessTokens(
        ref.watch(tokenSessionControllerProvider(key)),
      ),
    );

/// Detects whether an invite's relay uses token auth (NIP-11
/// `kuzz_token_auth`): a descriptor, `null` for a legacy relay, or a throw
/// ([TokenAuthDetectionException]) when the relay cannot be asked.
final inviteTokenAuthDetectorProvider =
    Provider<Future<TokenAuthDescriptor?> Function(String origin)>(
      (ref) =>
          (origin) => fetchTokenAuthDescriptor(
            origin,
            ref.read(authHttpClientProvider),
          ),
    );

const _unset = Object();

/// Finishes setup after an invite membership claim. Public channels are not
/// joined automatically; members join them from the channel directory.
abstract interface class InviteJoinRecovery {
  /// Finishes setup and returns the channel to focus, if any.
  Future<String?> ensureStarterChannels();
}

/// The active relay and signing identity a recovery instance must be bound to.
class InviteJoinRecoveryScope {
  const InviteJoinRecoveryScope({
    required this.relayHttpOrigin,
    required this.communityId,
    required this.nsec,
  });

  /// Canonical HTTP(S) origin used by the active relay session.
  final String relayHttpOrigin;

  /// Account that must remain active: several can share one origin.
  final String communityId;

  /// Signing identity that must remain active for the recovery lifetime.
  final String? nsec;
}

/// Constructs a fresh recovery operation for one scoped setup attempt.
typedef InviteJoinRecoveryFactory =
    InviteJoinRecovery Function(InviteJoinRecoveryScope scope);

/// Provides recovery construction after the active community is authenticated.
final inviteJoinRecoveryProvider = Provider<InviteJoinRecoveryFactory>((ref) {
  throw StateError(
    'inviteJoinRecoveryProvider must be configured by the app root',
  );
});

enum InviteJoinStatus {
  idle,
  confirming,
  claiming,
  success,
  switchedExisting,
  error,
}

class InviteJoinState {
  final InviteJoinStatus status;
  final InviteDeepLink? invite;
  final String? host;
  final String? communityName;
  final String? errorMessage;
  final bool requiresFreshInvite;
  final bool isStarterSetupRecovery;
  final String? focusChannelId;

  /// Canonical origin when the invite's relay uses token auth (NIP-11
  /// `kuzz_token_auth`): joining requires signing in, never a new key.
  final String? tokenRelayOrigin;

  /// Whether [tokenRelayOrigin] reflects a completed detection. A detection
  /// outage leaves it unchecked so confirmation retries it rather than
  /// falling back to a legacy keypair.
  final bool tokenAuthChecked;

  /// The invite is for a server other than the built-in one. Nothing was
  /// added or switched, and it cannot be confirmed.
  final bool differentService;

  const InviteJoinState({
    this.status = InviteJoinStatus.idle,
    this.invite,
    this.host,
    this.communityName,
    this.errorMessage,
    this.requiresFreshInvite = false,
    this.isStarterSetupRecovery = false,
    this.focusChannelId,
    this.tokenRelayOrigin,
    this.tokenAuthChecked = false,
    this.differentService = false,
  });

  InviteJoinState copyWith({
    InviteJoinStatus? status,
    InviteDeepLink? invite,
    String? host,
    String? communityName,
    Object? errorMessage = _unset,
    bool? requiresFreshInvite,
    bool? isStarterSetupRecovery,
    Object? focusChannelId = _unset,
    Object? tokenRelayOrigin = _unset,
    bool? tokenAuthChecked,
  }) => InviteJoinState(
    status: status ?? this.status,
    invite: invite ?? this.invite,
    host: host ?? this.host,
    communityName: communityName ?? this.communityName,
    errorMessage: identical(errorMessage, _unset)
        ? this.errorMessage
        : errorMessage as String?,
    requiresFreshInvite: requiresFreshInvite ?? this.requiresFreshInvite,
    isStarterSetupRecovery:
        isStarterSetupRecovery ?? this.isStarterSetupRecovery,
    focusChannelId: identical(focusChannelId, _unset)
        ? this.focusChannelId
        : focusChannelId as String?,
    tokenRelayOrigin: identical(tokenRelayOrigin, _unset)
        ? this.tokenRelayOrigin
        : tokenRelayOrigin as String?,
    tokenAuthChecked: tokenAuthChecked ?? this.tokenAuthChecked,
    differentService: differentService,
  );
}

class InviteJoinNotifier extends Notifier<InviteJoinState> {
  Community? _pendingStarterSetupCommunity;
  Future<void>? _starterSetupInFlight;

  @override
  InviteJoinState build() => const InviteJoinState();

  /// With a built-in server, only its invites are accepted (same host and
  /// port). Any other host adds and switches nothing.
  bool _isDifferentService(InviteDeepLink invite) => !isServerAllowed(
    invite.relayUrl,
    builtInOrigin: ref.read(defaultCommunityOriginProvider),
  );

  InviteJoinState _differentServiceState(InviteDeepLink invite) =>
      InviteJoinState(
        status: InviteJoinStatus.error,
        invite: invite,
        errorMessage: differentServiceInviteMessage,
        requiresFreshInvite: true,
        differentService: true,
      );

  /// Invite copy names the server only when there is no built-in one.
  String _signInToUseInvite(String communityName) =>
      ref.read(hasBuiltInServerProvider)
      ? appL10n.inviteSignInToUse
      : appL10n.inviteSignInToCommunityToUse(communityName);

  Future<void> prepare(InviteDeepLink invite) async {
    validateInviteRelayUri(Uri.parse(invite.relayUrl));
    if (_isDifferentService(invite)) {
      _pendingStarterSetupCommunity = null;
      state = _differentServiceState(invite);
      return;
    }
    final existing = await _inviteCommunity(invite.relayUrl);
    if (existing?.signedOut == true) {
      _pendingStarterSetupCommunity = null;
      state = InviteJoinState(
        status: InviteJoinStatus.error,
        invite: invite,
        host: _hostFromRelay(invite.relayUrl),
        communityName: existing!.name,
        errorMessage: appL10n.inviteSignInAgainFirst,
      );
      return;
    }
    if (existing != null &&
        (existing.tokenAuth || existing.googleBackupAccountId != null)) {
      // Google sign-in/recovery does not grant community membership. Claim
      // with the existing signing key (or old token principal) on confirmation.
      _pendingStarterSetupCommunity = null;
      state = InviteJoinState(
        status: InviteJoinStatus.confirming,
        invite: invite,
        host: _hostFromRelay(invite.relayUrl),
        communityName: existing.name,
      );
      return;
    }
    if (existing != null) {
      await ref
          .read(communityListProvider.notifier)
          .switchCommunity(existing.id);
      if (existing.starterSetupIncomplete) {
        _pendingStarterSetupCommunity = existing;
        state = InviteJoinState(
          status: InviteJoinStatus.claiming,
          invite: invite,
          host: _hostFromRelay(invite.relayUrl),
          communityName: existing.name,
          isStarterSetupRecovery: true,
        );
        return;
      }
      _pendingStarterSetupCommunity = null;
      state = InviteJoinState(
        status: InviteJoinStatus.switchedExisting,
        invite: invite,
        host: _hostFromRelay(invite.relayUrl),
        communityName: existing.name,
      );
      return;
    }

    _pendingStarterSetupCommunity = null;
    String? tokenRelayOrigin;
    var tokenAuthChecked = false;
    try {
      tokenRelayOrigin = await _detectTokenRelay(invite.relayUrl);
      tokenAuthChecked = true;
    } on TokenAuthDetectionException catch (error) {
      // Re-checked on confirmation; never assumed legacy.
      debugPrint('[InviteJoin] token auth detection deferred: $error');
    }
    state = InviteJoinState(
      status: InviteJoinStatus.confirming,
      invite: invite,
      host: _hostFromRelay(invite.relayUrl),
      communityName: Community.nameFromUrl(invite.relayUrl),
      tokenRelayOrigin: tokenRelayOrigin,
      tokenAuthChecked: tokenAuthChecked,
    );
  }

  /// The canonical origin when [relayUrl] is a token relay, else `null`.
  Future<String?> _detectTokenRelay(String relayUrl) async {
    final origin = normalizeRelayOrigin(relayUrl);
    final descriptor = await ref.read(inviteTokenAuthDetectorProvider)(origin);
    return descriptor == null || descriptor.keyBackup ? null : origin;
  }

  Future<void> confirmJoin() async {
    final invite = state.invite;
    if (invite != null && _isDifferentService(invite)) {
      _pendingStarterSetupCommunity = null;
      state = _differentServiceState(invite);
      return;
    }
    if (invite == null ||
        state.requiresFreshInvite ||
        (state.status != InviteJoinStatus.confirming &&
            state.status != InviteJoinStatus.error)) {
      return;
    }

    state = state.copyWith(
      status: InviteJoinStatus.claiming,
      errorMessage: null,
      requiresFreshInvite: false,
    );
    try {
      final existing = await _inviteCommunity(invite.relayUrl);
      if (existing?.signedOut == true) {
        throw InviteClaimException(
          appL10n.inviteSignInAgainFirst,
          userFacing: true,
        );
      }
      if (existing != null && existing.tokenAuth) {
        final wasActive =
            (await ref.read(activeCommunityProvider.future))?.id == existing.id;
        final session = SessionKey.ofCommunity(existing);
        if (session == null) {
          throw InviteClaimException(
            _signInToUseInvite(existing.name),
            userFacing: true,
          );
        }
        await _claimWithBearer(
          invite,
          tokens: ref.read(inviteClaimAccessTokensProvider(session)),
          communityName: existing.name,
        );
        await ref
            .read(communityListProvider.notifier)
            .switchCommunity(existing.id);
        // Switching to the already-active community is a no-op, so its
        // session would stay parked on the pre-claim "not a relay member"
        // AUTH deny.
        if (wasActive) _reconnectParkedSession();
        state = state.copyWith(
          status: InviteJoinStatus.switchedExisting,
          communityName: existing.name,
          isStarterSetupRecovery: false,
        );
        return;
      }
      if (existing != null) {
        if (existing.googleBackupAccountId != null) {
          final nsec = existing.nsec;
          if (nsec == null || pubkeyFromNsec(nsec) != existing.pubkey) {
            throw InviteClaimException(appL10n.inviteRecoverKeyFirst);
          }
          await _claimWithSigningKey(invite, nsec);
        }
        await ref
            .read(communityListProvider.notifier)
            .switchCommunity(existing.id);
        if (existing.googleBackupAccountId != null) _reconnectParkedSession();
        if (existing.starterSetupIncomplete || state.isStarterSetupRecovery) {
          _pendingStarterSetupCommunity = existing;
          await startStarterSetupRecovery();
        } else {
          state = state.copyWith(
            status: InviteJoinStatus.switchedExisting,
            communityName: existing.name,
            isStarterSetupRecovery: false,
          );
        }
        return;
      }

      if (state.isStarterSetupRecovery) {
        state = state.copyWith(
          status: InviteJoinStatus.error,
          errorMessage: appL10n.inviteCommunityUnavailable,
        );
        return;
      }

      var tokenRelayOrigin = state.tokenRelayOrigin;
      if (!state.tokenAuthChecked) {
        tokenRelayOrigin = await _detectTokenRelay(invite.relayUrl);
        state = state.copyWith(
          tokenRelayOrigin: tokenRelayOrigin,
          tokenAuthChecked: true,
        );
      }
      if (tokenRelayOrigin != null) {
        await _joinTokenRelay(invite, tokenRelayOrigin);
        return;
      }

      final keys = ref.read(inviteKeyGeneratorProvider)();
      final claim = await _claimWithSigningKey(invite, keys.nsec);

      final community = Community.create(
        name: _communityNameFromClaim(claim, invite.relayUrl),
        relayUrl: invite.relayUrl,
        pubkey: keys.public,
        nsec: keys.nsec,
        sensitiveActionPolicy: SensitiveActionPolicy.disabledByUser,
        starterSetupIncomplete: true,
      );
      await ref
          .read(authProvider.notifier)
          .authenticateWithCommunity(community);
      _pendingStarterSetupCommunity = community;
      state = state.copyWith(isStarterSetupRecovery: true);
      await startStarterSetupRecovery();
    } catch (error) {
      final requiresFreshInvite = _requiresFreshInvite(error);
      state = state.copyWith(
        status: InviteJoinStatus.error,
        errorMessage: _friendlyInviteError(error),
        requiresFreshInvite: requiresFreshInvite,
      );
    }
  }

  Future<Map<String, dynamic>> _claimWithSigningKey(
    InviteDeepLink invite,
    String nsec,
  ) async {
    final body = jsonEncode({
      'code': invite.code,
      if (invite.policyReceipt != null) 'policy_receipt': invite.policyReceipt,
    });
    validateInviteRelayUri(Uri.parse(invite.relayUrl));
    final url = _claimUrlFromRelay(invite.relayUrl);
    final authorization = await buildNip98AuthHeader(
      method: 'POST',
      url: url,
      bodyBytes: utf8.encode(body),
      nsec: nsec,
    );
    final request = http.Request('POST', Uri.parse(url))
      ..followRedirects = false
      ..headers.addAll({
        'Authorization': authorization,
        'Content-Type': 'application/json',
      })
      ..body = body;
    final response = await http.Response.fromStream(
      await ref.read(inviteJoinHttpClientProvider).send(request),
    );
    final decoded = jsonDecode(response.body.isEmpty ? '{}' : response.body);
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw InviteClaimException(
        decoded is Map && decoded['error'] is String
            ? decoded['error'] as String
            : 'HTTP ${response.statusCode}',
      );
    }
    if (decoded is! Map) {
      throw const FormatException('Invite claim returned malformed JSON');
    }
    return Map<String, dynamic>.from(decoded);
  }

  /// Joins a token relay this device has no community for: signs the
  /// principal in (system browser) BEFORE claiming, claims with its bearer,
  /// then saves a token community for that principal, which takes the session
  /// over from the sign-in attempt. Never creates a key.
  Future<void> _joinTokenRelay(InviteDeepLink invite, String origin) async {
    final displayName = Community.nameFromUrl(invite.relayUrl);
    final attempt = ref.read(tokenSignInAttemptProvider)(origin);
    final Community community;
    try {
      final token = await attempt.signIn()
          ? await attempt.ensureFreshAccessToken()
          : null;
      final principalId = attempt.state.principalId;
      if (token == null || principalId == null) {
        throw InviteClaimException(
          _signInToUseInvite(displayName),
          userFacing: true,
        );
      }
      final claim = await _claimWithBearer(
        invite,
        tokens: ControllerRelayAccessTokens(attempt),
        communityName: displayName,
      );
      community = Community.create(
        name: _communityNameFromClaim(claim, invite.relayUrl),
        relayUrl: invite.relayUrl,
        pubkey: principalId,
        starterSetupIncomplete: true,
        tokenAuth: true,
      );
      await ref
          .read(authProvider.notifier)
          .authenticateWithCommunity(community, attempt: attempt);
    } finally {
      // Handed over, this ends nothing: the attempt holds no tokens anymore.
      await attempt.discard();
      attempt.dispose();
    }
    _pendingStarterSetupCommunity = community;
    state = state.copyWith(isStarterSetupRecovery: true);
    await startStarterSetupRecovery();
  }

  /// Reconnects the live relay session unless it is connected (e.g. parked
  /// on a member-restricted AUTH deny that the claim just lifted).
  void _reconnectParkedSession() {
    if (!ref.exists(relaySessionProvider)) return;
    if (ref.read(relaySessionProvider).status == SessionStatus.connected) {
      return;
    }
    unawaited(ref.read(relaySessionProvider.notifier).reconnect());
  }

  /// Claims [invite] for the token principal [tokens] belongs to
  /// (`already_member` is a success) and returns the claim body. Throws when
  /// the relay rejects it.
  Future<Map<String, dynamic>> _claimWithBearer(
    InviteDeepLink invite, {
    required RelayAccessTokens tokens,
    required String communityName,
  }) async {
    final relayUri = Uri.parse(invite.relayUrl);
    validateInviteRelayUri(relayUri);
    final url = Uri.parse(_claimUrlFromRelay(invite.relayUrl));
    final body = jsonEncode({
      'code': invite.code,
      if (invite.policyReceipt != null) 'policy_receipt': invite.policyReceipt,
    });
    final client = ref.read(inviteJoinHttpClientProvider);
    final http.Response response;
    try {
      response = await sendWithBearer(tokens, (authorization) async {
        final request = http.Request('POST', url)
          ..followRedirects = false
          ..headers.addAll({
            'Authorization': authorization,
            'Content-Type': 'application/json',
          })
          ..body = body;
        return http.Response.fromStream(await client.send(request));
      });
    } on RelayTokenUnavailableException {
      throw InviteClaimException(
        _signInToUseInvite(communityName),
        userFacing: true,
      );
    }
    if (response.statusCode < 200 || response.statusCode >= 300) {
      final decoded = _tryDecode(response.body);
      throw InviteClaimException(
        decoded is Map && decoded['error'] is String
            ? decoded['error'] as String
            : 'HTTP ${response.statusCode}',
      );
    }
    final decoded = _tryDecode(response.body);
    return decoded is Map ? Map<String, dynamic>.from(decoded) : const {};
  }

  /// Runs the pending recovery after its progress UI has been presented.
  Future<void> startStarterSetupRecovery() async {
    final community = _pendingStarterSetupCommunity;
    if (community == null ||
        state.status != InviteJoinStatus.claiming ||
        !state.isStarterSetupRecovery) {
      return;
    }
    final inFlight = _starterSetupInFlight;
    if (inFlight != null) return inFlight;

    final setup = _finishStarterSetup(community);
    _starterSetupInFlight = setup;
    try {
      await setup;
    } finally {
      if (identical(_starterSetupInFlight, setup)) {
        _starterSetupInFlight = null;
      }
    }
  }

  /// The account an invite for [relayUrl] is claimed with: the active one
  /// when it is on that server, otherwise the one used most recently there.
  Future<Community?> _inviteCommunity(String relayUrl) async {
    final matching = _communitiesOfRelay(
      await ref.read(communityListProvider.future),
      relayUrl,
    );
    if (matching.length < 2) return matching.firstOrNull;
    final activeId = await ref.read(communityStorageProvider).loadActiveId();
    return matching.where((row) => row.id == activeId).firstOrNull ??
        mostRecentlyUsedCommunity(matching) ??
        matching.first;
  }

  Future<void> _finishStarterSetup(Community community) async {
    try {
      // Authentication and community switches invalidate the scoped providers.
      // Wait for the active community projection to settle before constructing
      // the recovery, otherwise it could capture the previous relay's actions.
      final activeCommunity = await ref.read(activeCommunityProvider.future);
      if (activeCommunity?.id != community.id ||
          activeCommunity?.relayUrl != community.relayUrl ||
          activeCommunity?.nsec != community.nsec) {
        throw StateError('Active community changed before invite recovery');
      }
      final config = RelayConfig(
        baseUrl: community.relayUrl,
        nsec: community.nsec,
      );
      final focusChannelId = await ref
          .read(inviteJoinRecoveryProvider)(
            InviteJoinRecoveryScope(
              relayHttpOrigin: config.baseUrl,
              communityId: community.id,
              nsec: config.nsec,
            ),
          )
          .ensureStarterChannels();
      await _saveStarterSetupState(community, incomplete: false);
      state = state.copyWith(
        status: InviteJoinStatus.success,
        communityName: community.name,
        isStarterSetupRecovery: false,
        focusChannelId: focusChannelId,
      );
    } catch (error) {
      Object visibleError = error;
      try {
        await _saveStarterSetupState(community, incomplete: true);
      } catch (storageError) {
        visibleError = storageError;
      }
      state = state.copyWith(
        status: InviteJoinStatus.error,
        errorMessage: _friendlyStarterSetupError(visibleError),
        requiresFreshInvite: false,
        isStarterSetupRecovery: true,
        focusChannelId: null,
      );
    }
  }

  Future<void> _saveStarterSetupState(
    Community community, {
    required bool incomplete,
  }) {
    return ref.read(communityTransitionProvider).runExclusive(() async {
      final storage = ref.read(communityStorageProvider);
      final current = (await storage.loadAll())
          .where((item) => item.id == community.id)
          .firstOrNull;
      if (current == null ||
          current.signedOut ||
          current.pubkey != community.pubkey) {
        throw InviteClaimException(
          appL10n.inviteSignInAgainFirst,
          userFacing: true,
        );
      }
      final updated = current.copyWith(starterSetupIncomplete: incomplete);
      await storage.save(updated);
      ref.invalidate(communityListProvider);
      ref.invalidate(activeCommunityProvider);
      ref.invalidate(authProvider);
    });
  }

  void reset() {
    _pendingStarterSetupCommunity = null;
    state = const InviteJoinState();
  }
}

final inviteJoinProvider =
    NotifierProvider<InviteJoinNotifier, InviteJoinState>(
      InviteJoinNotifier.new,
    );

class InviteClaimException implements Exception {
  final String message;

  /// [message] is a translated sentence shown to the user as is.
  final bool userFacing;

  const InviteClaimException(this.message, {this.userFacing = false});

  @override
  String toString() => message;
}

List<Community> _communitiesOfRelay(
  List<Community> communities,
  String relayUrl,
) {
  final invite = _relayOriginForComparison(relayUrl);
  return [
    for (final community in communities)
      if (_relayOriginForComparison(community.relayUrl) case final current?
          when current == invite)
        community,
  ];
}

({bool secure, String host, int? port})? _relayOriginForComparison(String url) {
  final uri = Uri.tryParse(url);
  if (uri == null || uri.host.isEmpty) return null;
  final secure = switch (uri.scheme) {
    'https' || 'wss' => true,
    'http' || 'ws' => false,
    _ => null,
  };
  if (secure == null) return null;
  return (
    secure: secure,
    host: uri.host.toLowerCase(),
    port: _effectivePort(uri),
  );
}

int? _effectivePort(Uri uri) {
  if (uri.hasPort) return uri.port;
  return switch (uri.scheme) {
    'https' || 'wss' => 443,
    'http' || 'ws' => 80,
    _ => null,
  };
}

String _hostFromRelay(String relayUrl) {
  final uri = Uri.parse(relayUrl);
  if (uri.hasPort) return '${uri.host}:${uri.port}';
  return uri.host;
}

String _claimUrlFromRelay(String relayUrl) {
  final uri = Uri.parse(relayUrl);
  final scheme = switch (uri.scheme) {
    'wss' => 'https',
    'ws' => 'http',
    _ => throw FormatException('Invalid relay URL scheme: ${uri.scheme}'),
  };
  return Uri(
    scheme: scheme,
    host: uri.host,
    port: uri.hasPort ? uri.port : null,
    path: '/api/invites/claim',
  ).toString();
}

Object? _tryDecode(String body) {
  try {
    return jsonDecode(body.isEmpty ? '{}' : body);
  } on FormatException {
    return null;
  }
}

String _communityNameFromClaim(Map<String, dynamic> claim, String relayUrl) {
  final host = claim['host'];
  if (host is String && host.trim().isNotEmpty) return host.trim();
  return Community.nameFromUrl(relayUrl);
}

bool _requiresFreshInvite(Object error) {
  final message = error.toString();
  return message.contains('join_policy_required') ||
      message.contains('invite_exhausted');
}

String _friendlyInviteError(Object error) {
  final message = error.toString();
  if (error is InviteClaimException && error.userFacing ||
      message.startsWith('Sign in to ')) {
    return message;
  }
  if (error is TokenAuthDetectionException) {
    return appL10n.inviteRelayUnreachable;
  }
  if (message.contains('invite_expired')) return appL10n.inviteExpired;
  if (message.contains('invite_exhausted')) {
    return appL10n.inviteExhausted;
  }
  if (message.contains('invite_invalid')) return appL10n.inviteInvalid;
  if (message.contains('join_policy_required')) {
    return appL10n.inviteApprovalExpired;
  }
  if (message.contains('SocketException') ||
      message.contains('Connection refused') ||
      message.contains('Network is unreachable') ||
      message.contains('No route to host')) {
    return appL10n.inviteRelayUnreachable;
  }
  return appL10n.inviteJoinFailed(message);
}

String _friendlyStarterSetupError(Object error) {
  final message = error.toString();
  if (message.contains('SocketException') ||
      message.contains('Connection refused') ||
      message.contains('Network is unreachable') ||
      message.contains('No route to host')) {
    return appL10n.inviteStarterSetupUnreachable;
  }
  return appL10n.inviteStarterSetupFailed;
}
