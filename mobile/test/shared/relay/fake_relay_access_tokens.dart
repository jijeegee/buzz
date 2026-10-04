import 'package:buzz/shared/auth/token/token_session.dart';
import 'package:buzz/shared/relay/relay_access_tokens.dart';

/// Scriptable [RelayAccessTokens] for transport tests.
class FakeRelayAccessTokens implements RelayAccessTokens {
  FakeRelayAccessTokens({this.token = 'bzs_1', List<String?>? rotations})
    : rotations = rotations ?? [];

  /// The token [current] and [fresh] hand out.
  String? token;

  /// Tokens [afterExpired] moves to, in order; `null` when exhausted.
  final List<String?> rotations;

  final List<String> expiredCalls = [];
  int freshCalls = 0;
  final List<void Function(String)> _tokenListeners = [];
  final List<void Function(TokenSessionState)> _stateListeners = [];

  int get tokenListenerCount => _tokenListeners.length;
  int get stateListenerCount => _stateListeners.length;

  /// Simulate a cached token inside its expiry margin: [current] is `null`
  /// while [fresh] still rotates to [token].
  bool currentStale = false;

  @override
  String? get current => currentStale ? null : token;

  @override
  Future<String?> fresh() async {
    freshCalls++;
    return token;
  }

  @override
  Future<String?> afterExpired(String rejected) async {
    expiredCalls.add(rejected);
    token = rotations.isEmpty ? null : rotations.removeAt(0);
    return token;
  }

  /// Simulate the session issuing [next] (a proactive rotation).
  void rotate(String next) {
    token = next;
    for (final listener in List.of(_tokenListeners)) {
      listener(next);
    }
  }

  /// Simulate a session status change.
  void emitState(TokenSessionState state) {
    if (state.status == TokenSessionStatus.signedOut) token = null;
    for (final listener in List.of(_stateListeners)) {
      listener(state);
    }
  }

  @override
  void Function() addTokenListener(void Function(String token) listener) {
    _tokenListeners.add(listener);
    return () => _tokenListeners.remove(listener);
  }

  @override
  void Function() addStateListener(
    void Function(TokenSessionState state) listener,
  ) {
    _stateListeners.add(listener);
    return () => _stateListeners.remove(listener);
  }
}
