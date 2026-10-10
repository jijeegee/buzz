import 'dart:async';

import 'package:flutter/foundation.dart';

import 'auth_api.dart';
import 'install_id_store.dart';
import 'oidc_login.dart';
import 'refresh_token_store.dart';
import 'web_auth_launcher.dart';

/// Wall clock, injectable for tests.
typedef SessionClock = DateTime Function();

/// One-shot timer factory, injectable for tests.
typedef SessionTimerFactory =
    Timer Function(Duration delay, void Function() callback);

/// Lifecycle of one relay's token session.
enum TokenSessionStatus {
  /// Reading the stored refresh token / first rotation.
  restoring,

  /// No session. Offer "Sign in with Google".
  signedOut,

  /// The system browser login is open.
  signingIn,

  /// A usable access token is in memory.
  signedIn,

  /// A transient refresh failure left no usable access token; a bounded
  /// backoff retry is armed.
  retrying,

  /// Retries are exhausted (or the keychain could not be read). The stored
  /// session is kept; the UI must offer retry and sign-in.
  stalled,
}

/// Immutable snapshot for the UI. The access token is deliberately not part
/// of it; read it from [TokenSessionController.currentAccessToken].
@immutable
class TokenSessionState {
  const TokenSessionState({
    required this.status,
    this.principalId,
    this.errorMessage,
    this.nextRetryAt,
  });

  final TokenSessionStatus status;
  final String? principalId;

  /// Why the last operation failed, for display.
  final String? errorMessage;

  /// When the armed backoff retry fires ([TokenSessionStatus.retrying]).
  final DateTime? nextRetryAt;

  TokenSessionState withError(String? message) => TokenSessionState(
    status: status,
    principalId: principalId,
    errorMessage: message,
    nextRetryAt: nextRetryAt,
  );

  @override
  bool operator ==(Object other) =>
      other is TokenSessionState &&
      other.status == status &&
      other.principalId == principalId &&
      other.errorMessage == errorMessage &&
      other.nextRetryAt == nextRetryAt;

  @override
  int get hashCode =>
      Object.hash(status, principalId, errorMessage, nextRetryAt);

  @override
  String toString() =>
      'TokenSessionState(${status.name}, principal=$principalId, '
      'error=$errorMessage, nextRetryAt=$nextRetryAt)';
}

/// A sign-out that could not reach the relay; the session is kept.
class TokenSessionException implements Exception {
  const TokenSessionException(this.message);

  final String message;

  @override
  String toString() => 'TokenSessionException: $message';
}

class _AccessToken {
  const _AccessToken(this.token, this.expiresAt);

  final String token;
  final DateTime expiresAt;
}

/// A queued store operation belonged to a session that has since ended.
class _StaleGeneration implements Exception {
  const _StaleGeneration();
}

/// Owns one relay origin's token session (plan §3.2 mobile row).
///
/// - The refresh token lives in [RefreshTokenStore] (keychain); the access
///   token only in memory.
/// - Every rotation persists the new refresh **before** the new access token
///   is published. A failed persist is terminal: the unsaved device session
///   is revoked best-effort and the user signs in again.
/// - Refresh is single-flight, and every result and store write is fenced by
///   a generation counter bumped on sign-in and sign-out, so a late rotation
///   can never resurrect a signed-out session (AGENTS.md rule 2).
/// - 400/401 from `/auth/refresh` sign out; transient failures back off
///   [retryDelays] and then stop in [TokenSessionStatus.stalled]
///   (AGENTS.md rule 4).
/// - [dispose] bumps the generation: a rotation still in flight is neither
///   persisted nor published. The controller is only disposed when its
///   session is being dropped (device-only community removal deletes the
///   stored refresh and then invalidates the controller) or the app is
///   exiting, so a late write would resurrect a removed session.
class TokenSessionController {
  TokenSessionController({
    required this.origin,
    required AuthApi api,
    required RefreshTokenStore store,
    required WebAuthLauncher launcher,
    SessionClock? clock,
    SessionTimerFactory? timerFactory,
    this.deviceName,
    InstallIdStore? installIds,
  }) : _installIds = installIds,
       _api = api,
       _store = store,
       _launcher = launcher,
       _clock = clock ?? DateTime.now,
       _timerFactory = timerFactory ?? Timer.new;

  /// Refresh this long before the access token expires.
  static const refreshLead = Duration(minutes: 5);

  /// Access tokens closer than this to expiry are not handed out.
  static const minRemaining = Duration(seconds: 60);

  /// Transient-failure backoff; after the last step the session stalls.
  static const retryDelays = [
    Duration(seconds: 5),
    Duration(seconds: 15),
    Duration(seconds: 60),
    Duration(seconds: 120),
    Duration(seconds: 300),
  ];

  /// Lower bound for proactive refresh timers.
  static const minTimerDelay = Duration(seconds: 30);

  final String origin;

  /// Shown in the relay's device list (`device_name`, ≤ 64 chars).
  final String? deviceName;

  /// This install's stable device id, sent with each sign-in.
  final InstallIdStore? _installIds;

  final AuthApi _api;
  final RefreshTokenStore _store;
  final WebAuthLauncher _launcher;
  final SessionClock _clock;
  final SessionTimerFactory _timerFactory;

  TokenSessionState _state = const TokenSessionState(
    status: TokenSessionStatus.restoring,
  );
  final List<void Function(TokenSessionState)> _listeners = [];

  int _generation = 0;
  String? _refreshToken;
  String? _principalId;
  String _identityMode = 'token';

  /// Account sessions for custody never select a messaging identity.
  String get identityMode => _identityMode;

  /// Changes on login, logout and disposal; custody results must stay in it.
  int get generation => _generation;
  _AccessToken? _access;
  int _failures = 0;
  Timer? _timer;
  bool _disposed = false;

  Future<void>? _restoring;
  Future<bool>? _signingIn;
  Future<String?>? _inflight;
  int _inflightGeneration = -1;
  Future<void> _storeQueue = Future<void>.value();

  TokenSessionState get state => _state;

  /// Listen to state changes; returns the unsubscribe function.
  void Function() addListener(void Function(TokenSessionState) listener) {
    _listeners.add(listener);
    return () => _listeners.remove(listener);
  }

  final List<void Function(String)> _tokenListeners = [];

  /// Listen for every newly issued access token (sign-in and each rotation,
  /// including signedIn → signedIn rotations that [addListener] does not
  /// report). Long-lived transports use it to re-AUTH before the old token's
  /// deadline. Returns the unsubscribe function.
  void Function() addAccessTokenListener(void Function(String) listener) {
    _tokenListeners.add(listener);
    return () => _tokenListeners.remove(listener);
  }

  /// The in-memory access token if it has more than [minRemaining] left.
  String? get currentAccessToken {
    final access = _access;
    if (_disposed || access == null) return null;
    final remaining = access.expiresAt.difference(_clock());
    return remaining > minRemaining ? access.token : null;
  }

  /// Restore the stored session (call once at start). Single-flight.
  Future<void> restore() {
    return _restoring ??= _runRestore().whenComplete(() => _restoring = null);
  }

  /// Run the Google (or [provider]) login. Returns whether it signed in;
  /// failures are reported in [state]. Single-flight.
  Future<bool> signIn({
    String provider = 'google',
    String identityMode = 'token',
  }) {
    return _signingIn ??= _runSignIn(
      provider,
      identityMode,
    ).whenComplete(() => _signingIn = null);
  }

  /// A usable access token, rotating when the cached one is missing or near
  /// expiry. `null` when signed out, or while a backoff retry/stall is in
  /// effect (the armed retry and [retryNow] are the only refresh triggers
  /// then, so callers in a reconnect loop cannot hammer the relay).
  Future<String?> ensureFreshAccessToken() async {
    if (_disposed) return null;
    final current = currentAccessToken;
    if (current != null) return current;
    final restoring = _restoring;
    if (restoring != null) {
      await restoring;
      return currentAccessToken;
    }
    final inflight = _joinableInflight;
    if (inflight != null) return inflight;
    if (_refreshToken == null || !_canRefreshOnDemand) return null;
    return _refresh();
  }

  /// The relay rejected [rejectedToken] with `token_expired`: rotate once,
  /// unless the session already moved past that token.
  Future<String?> refreshAfterTokenExpired(String rejectedToken) async {
    if (_disposed) return null;
    final current = currentAccessToken;
    if (current != null && current != rejectedToken) return current;
    final inflight = _joinableInflight;
    if (inflight != null) return inflight;
    if (_refreshToken == null || !_canRefreshOnDemand) return null;
    return _refresh();
  }

  /// The recovery affordance for [TokenSessionStatus.stalled] /
  /// [TokenSessionStatus.retrying]: reset the backoff and try now.
  Future<void> retryNow() async {
    if (_disposed) return;
    if (_refreshToken == null) return restore();
    _failures = 0;
    _cancelTimer();
    _emit(
      TokenSessionState(
        status: TokenSessionStatus.restoring,
        principalId: _principalId,
      ),
    );
    await _refresh();
  }

  /// Revoke this device on the relay, then forget the session locally.
  ///
  /// Throws [TokenSessionException] and keeps the session when the relay
  /// cannot be reached: forgetting locally while the device session stays
  /// live would strand a valid credential on the server.
  Future<void> signOut() async {
    if (_disposed) return;
    final generation = _generation;
    if (_refreshToken == null && _access == null) {
      await _forget();
      return;
    }
    var token =
        currentAccessToken ?? (_refreshToken == null ? null : await _refresh());
    for (var attempt = 0; ; attempt++) {
      if (generation != _generation) return;
      if (token == null) {
        throw _keepSession('Could not reach the relay to sign out. Try again.');
      }
      try {
        await _api.logout(token);
        break;
      } on AuthApiException catch (error) {
        if (generation != _generation) return;
        if (!error.isTerminal) {
          throw _keepSession(
            'Could not reach the relay to sign out. Try again.',
          );
        }
        if (error.code == 'token_expired' && attempt == 0) {
          token = await refreshAfterTokenExpired(token);
          continue;
        }
        // Revoked token or unknown device: the session is already dead.
        break;
      }
    }
    if (generation != _generation) return;
    await _forget();
  }

  /// Forget the session on this device only, without contacting the relay
  /// (the "remove from this device" recovery path).
  ///
  /// The delete runs through the same serialized store queue as rotation
  /// writes, after a generation bump, so a rotation write already in flight
  /// cannot land after it and resurrect the record. A failed delete
  /// propagates.
  Future<void> forgetOnDevice() => _forget(propagateDeleteFailure: true);

  /// Stop timers and notifications and fence in-flight rotations (see the
  /// class doc).
  void dispose() {
    _disposed = true;
    _generation += 1;
    _cancelTimer();
    _listeners.clear();
    _tokenListeners.clear();
  }

  // --- internals -----------------------------------------------------------

  bool _isCurrent(int generation) => !_disposed && generation == _generation;

  bool get _canRefreshOnDemand => switch (_state.status) {
    TokenSessionStatus.retrying ||
    TokenSessionStatus.stalled ||
    TokenSessionStatus.signedOut => false,
    _ => true,
  };

  Future<String?>? get _joinableInflight =>
      _inflightGeneration == _generation ? _inflight : null;

  Future<void> _runRestore() async {
    if (_disposed) return;
    final generation = _generation;
    _emit(
      TokenSessionState(
        status: TokenSessionStatus.restoring,
        principalId: _principalId,
      ),
    );
    final StoredTokenSession? stored;
    try {
      stored = await _store.read(origin);
    } catch (error) {
      if (!_isCurrent(generation)) return;
      debugPrint('Token session read failed for $origin: $error');
      _emit(
        const TokenSessionState(
          status: TokenSessionStatus.stalled,
          errorMessage: 'Could not read the saved sign-in. Try again.',
        ),
      );
      return;
    }
    if (!_isCurrent(generation)) return;
    if (stored == null) {
      _emit(const TokenSessionState(status: TokenSessionStatus.signedOut));
      return;
    }
    _refreshToken = stored.refreshToken;
    _principalId = stored.principalId;
    _identityMode = stored.identityMode;
    _failures = 0;
    await _refresh();
  }

  Future<String?> _refresh() {
    final inflight = _joinableInflight;
    if (inflight != null) return inflight;
    final generation = _generation;
    late final Future<String?> future;
    future = _runRefresh(generation).whenComplete(() {
      if (identical(_inflight, future)) _inflight = null;
    });
    _inflight = future;
    _inflightGeneration = generation;
    return future;
  }

  Future<String?> _runRefresh(int generation) async {
    final refresh = _refreshToken;
    final principal = _principalId;
    if (refresh == null || principal == null) return null;
    _cancelTimer();
    final RotatedTokens rotated;
    try {
      rotated = await _api.refresh(refresh);
    } on AuthApiException catch (error) {
      if (!_isCurrent(generation)) return currentAccessToken;
      if (error.isTerminal) {
        await _forget(errorMessage: _sessionEndedMessage(error));
        return null;
      }
      _onTransientFailure(error);
      return currentAccessToken;
    }
    // Persist before publish: the relay has consumed `refresh`.
    try {
      await _persist(
        generation,
        StoredTokenSession(
          refreshToken: rotated.refreshToken,
          principalId: principal,
          identityMode: _identityMode,
        ),
      );
    } on _StaleGeneration {
      return currentAccessToken;
    } catch (error) {
      debugPrint('Token session persist failed for $origin: $error');
      await _revokeUnsaved(rotated.accessToken);
      if (!_isCurrent(generation)) return currentAccessToken;
      await _forget(errorMessage: _unsavedMessage);
      return null;
    }
    if (!_isCurrent(generation)) return currentAccessToken;
    _refreshToken = rotated.refreshToken;
    _failures = 0;
    _publish(rotated.accessToken, rotated.expiresIn, principal);
    return rotated.accessToken;
  }

  Future<bool> _runSignIn(String provider, String identityMode) async {
    if (_disposed) return false;
    // Fence the login against a sign-out (or dispose) that happens while
    // the browser or the code exchange is still pending: such a grant must
    // never be persisted or published (AGENTS.md rule 2).
    final startGeneration = _generation;
    _emit(
      TokenSessionState(
        status: TokenSessionStatus.signingIn,
        principalId: _principalId,
      ),
    );
    final LoginGrant grant;
    try {
      final callbackScheme = await _launcher.callbackScheme();
      final installId = await _installIds?.readOrCreate();
      if (!_isCurrent(startGeneration)) return false;
      grant = await runOidcLogin(
        api: _api,
        launcher: _launcher,
        provider: provider,
        deviceName: deviceName,
        installId: installId,
        identityMode: identityMode,
        callbackScheme: callbackScheme,
      );
    } on WebAuthCancelledException {
      if (_isCurrent(startGeneration)) _emit(_settledState(null));
      return false;
    } on WebAuthBusyException {
      if (_isCurrent(startGeneration)) {
        _emit(
          _settledState('Another sign-in is open. Finish or close it first.'),
        );
      }
      return false;
    } on OidcLoginException catch (error) {
      if (_isCurrent(startGeneration)) _emit(_settledState(error.message));
      return false;
    } on AuthApiException catch (error) {
      if (_isCurrent(startGeneration)) {
        _emit(_settledState(_loginFailureMessage(error)));
      }
      return false;
    } catch (_) {
      if (_isCurrent(startGeneration)) {
        _emit(_settledState('Could not open the sign-in page. Try again.'));
      }
      return false;
    }
    if (!_isCurrent(startGeneration)) {
      await _revokeUnsaved(grant.accessToken);
      return false;
    }
    if (grant.identityMode != identityMode) {
      await _revokeUnsaved(grant.accessToken);
      if (!_isCurrent(startGeneration)) return false;
      _emit(
        _settledState(
          'This Google account uses a different identity mode. '
          'Existing account data has been preserved.',
        ),
      );
      return false;
    }
    // A new session replaces whatever was here; fence everything older.
    _generation += 1;
    final generation = _generation;
    _cancelTimer();
    _failures = 0;
    try {
      await _persist(
        generation,
        StoredTokenSession(
          refreshToken: grant.refreshToken,
          principalId: grant.principalId,
          identityMode: grant.identityMode,
        ),
      );
    } on _StaleGeneration {
      return false;
    } catch (error) {
      debugPrint('Token session persist failed for $origin: $error');
      await _revokeUnsaved(grant.accessToken);
      if (_isCurrent(generation)) await _forget(errorMessage: _unsavedMessage);
      return false;
    }
    if (!_isCurrent(generation)) return false;
    _refreshToken = grant.refreshToken;
    _principalId = grant.principalId;
    _identityMode = grant.identityMode;
    _publish(grant.accessToken, grant.expiresIn, grant.principalId);
    return true;
  }

  void _publish(String token, Duration expiresIn, String principal) {
    _access = _AccessToken(token, _clock().add(expiresIn));
    final lead = expiresIn - refreshLead;
    _armTimer(lead < minTimerDelay ? minTimerDelay : lead);
    _emit(
      TokenSessionState(
        status: TokenSessionStatus.signedIn,
        principalId: principal,
      ),
    );
    if (_disposed) return;
    for (final listener in List.of(_tokenListeners)) {
      listener(token);
    }
  }

  void _onTransientFailure(AuthApiException error) {
    if (_failures >= retryDelays.length) {
      _cancelTimer();
      _emit(
        TokenSessionState(
          status: TokenSessionStatus.stalled,
          principalId: _principalId,
          errorMessage: 'Could not reach the relay. ${error.message}',
        ),
      );
      return;
    }
    final delay = retryDelays[_failures];
    _failures += 1;
    _armTimer(delay);
    if (currentAccessToken != null) return;
    _access = null;
    _emit(
      TokenSessionState(
        status: TokenSessionStatus.retrying,
        principalId: _principalId,
        errorMessage: error.message,
        nextRetryAt: _clock().add(delay),
      ),
    );
  }

  /// Where a failed/cancelled sign-in lands, from the current facts (a
  /// timer may have changed them while the browser was open).
  TokenSessionState _settledState(String? errorMessage) {
    final TokenSessionStatus status;
    if (currentAccessToken != null) {
      status = TokenSessionStatus.signedIn;
    } else if (_refreshToken == null) {
      status = TokenSessionStatus.signedOut;
    } else if (_timer?.isActive ?? false) {
      status = TokenSessionStatus.retrying;
    } else {
      status = TokenSessionStatus.stalled;
    }
    return TokenSessionState(
      status: status,
      principalId: status == TokenSessionStatus.signedOut ? null : _principalId,
      errorMessage: errorMessage,
    );
  }

  Future<void> _forget({
    String? errorMessage,
    bool propagateDeleteFailure = false,
  }) async {
    _generation += 1;
    final generation = _generation;
    _cancelTimer();
    _access = null;
    _refreshToken = null;
    _principalId = null;
    _failures = 0;
    _emit(
      TokenSessionState(
        status: TokenSessionStatus.signedOut,
        errorMessage: errorMessage,
      ),
    );
    try {
      await _serialized(() async {
        if (generation != _generation) return;
        await _store.delete(origin);
      });
    } catch (error) {
      // The relay-side session is already gone, so the leftover record can
      // only fail its next refresh; still tell the user.
      debugPrint('Token session delete failed for $origin: $error');
      if (_isCurrent(generation)) {
        _emit(
          _state.withError(
            errorMessage ??
                'Signed out, but the saved sign-in could not be removed '
                    'from this device.',
          ),
        );
      }
      if (propagateDeleteFailure) rethrow;
    }
  }

  /// Best-effort revoke of a session whose refresh could not be saved. If
  /// this also fails, the relay's refresh-reuse detection revokes it when
  /// the stale refresh is next presented.
  Future<void> _revokeUnsaved(String accessToken) async {
    try {
      await _api.logout(accessToken);
    } on AuthApiException catch (error) {
      debugPrint('Could not revoke unsaved session on $origin: $error');
    }
  }

  Future<void> _persist(int generation, StoredTokenSession session) =>
      _serialized(() async {
        if (generation != _generation) throw const _StaleGeneration();
        await _store.write(origin, session);
      });

  /// Store operations run one at a time, each re-checking its generation
  /// when it actually runs, so a sign-out delete cannot be overtaken by a
  /// stale write queued earlier.
  Future<T> _serialized<T>(Future<T> Function() operation) {
    final result = _storeQueue.then((_) => operation());
    _storeQueue = result.then<void>((_) {}, onError: (Object _) {});
    return result;
  }

  void _armTimer(Duration delay) {
    _cancelTimer();
    if (_disposed) return;
    final generation = _generation;
    _timer = _timerFactory(delay, () {
      _timer = null;
      if (_isCurrent(generation)) unawaited(_refresh());
    });
  }

  void _cancelTimer() {
    _timer?.cancel();
    _timer = null;
  }

  TokenSessionException _keepSession(String message) {
    _emit(_state.withError(message));
    return TokenSessionException(message);
  }

  void _emit(TokenSessionState next) {
    if (_disposed || next == _state) return;
    _state = next;
    for (final listener in List.of(_listeners)) {
      listener(next);
    }
  }

  static const _unsavedMessage =
      'Could not save the sign-in on this device. Sign in again.';

  static String _sessionEndedMessage(AuthApiException error) =>
      switch (error.code) {
        'principal_disabled' => 'This account has been disabled.',
        _ => 'Your session has ended. Sign in again.',
      };

  static String _loginFailureMessage(AuthApiException error) =>
      switch (error.code) {
        'invalid_grant' => 'The sign-in expired. Try again.',
        'limit_reached' => 'Too many devices are signed in. Remove one first.',
        _ =>
          error.isTerminal
              ? 'Sign-in was rejected (${error.message}).'
              : 'Could not reach the relay. Try again.',
      };
}
