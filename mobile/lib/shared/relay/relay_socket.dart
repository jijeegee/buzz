import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:web_socket_channel/io.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

import 'nostr_models.dart';
import 'relay_access_tokens.dart';

/// Low-level websocket connection with NIP-42 or token authentication.
///
/// Handles the raw websocket lifecycle: connect, authenticate (NIP-42
/// challenge/response, or `["AUTH", {"token": ...}]` when [RelaySocket.
/// accessTokens] is set — plan §3.4), send/receive JSON frames, and
/// disconnect.
///
/// Does NOT handle reconnection — that is [RelaySessionNotifier]'s job.
enum SocketState { disconnected, connecting, authenticating, connected }

class RelayAuthRejectedException implements Exception {
  final String message;

  const RelayAuthRejectedException(this.message);

  @override
  String toString() => 'Relay authentication rejected: $message';
}

/// The relay rejected this socket's first token AUTH and a refresh produced
/// a new token. The relay has marked the connection failed (it refuses any
/// later AUTH on it), so the session must reconnect on a NEW socket, which
/// presents the refreshed token.
class RelayTokenRefreshedException implements Exception {
  const RelayTokenRefreshedException();

  @override
  String toString() =>
      'RelayTokenRefreshedException: token refreshed, reconnect required';
}

Exception classifyRelayAuthFailure(String message) {
  if (message.startsWith('error:')) return Exception(message);
  // The token authority is briefly unreachable: retry with backoff.
  if (message == 'auth-required: unavailable') return Exception(message);
  return RelayAuthRejectedException(message);
}

class RelaySocket {
  /// Interval for sending a ping and awaiting its pong before disconnecting.
  static const pingInterval = Duration(seconds: 30);

  @visibleForTesting
  static Duration debugPingInterval = pingInterval;

  final String _wsUrl;
  final String? _nsec;
  final void Function(List<dynamic> message) _onMessage;
  final void Function() _onConnected;
  final void Function(Object? error) _onDisconnected;

  WebSocketChannel? _channel;
  StreamSubscription<dynamic>? _subscription;
  SocketState _state = SocketState.disconnected;
  Completer<void>? _authCompleter;
  Timer? _authTimeout;
  String? _pendingAuthEventId;

  /// The token source for a token-auth community; `null` keeps NIP-42.
  /// Set before [connect].
  RelayAccessTokens? accessTokens;

  /// Whether a refreshable first-AUTH rejection refreshes the token (and
  /// fails with [RelayTokenRefreshedException]) or is terminal. The session
  /// clears it on the reconnect a refresh triggered, bounding the cycle to
  /// one refresh. Set before [connect].
  bool refreshRejectedToken = true;

  /// Bumped on every connection reset; fences awaited token reads.
  int _epoch = 0;
  bool _tokenAuthStarted = false;
  String? _pendingAuthToken;
  String? _queuedReauthToken;
  String? _boundToken;
  bool _tokenRefreshUsed = false;

  SocketState get state => _state;

  RelaySocket({
    required String wsUrl,
    required String? nsec,
    required void Function(List<dynamic> message) onMessage,
    required void Function() onConnected,
    required void Function(Object? error) onDisconnected,
  }) : _wsUrl = wsUrl,
       _nsec = nsec,
       _onMessage = onMessage,
       _onConnected = onConnected,
       _onDisconnected = onDisconnected;

  /// Connect to the relay and complete NIP-42 authentication.
  Future<void> connect() async {
    if (_state != SocketState.disconnected) return;
    _state = SocketState.connecting;

    try {
      _channel = IOWebSocketChannel.connect(
        Uri.parse(_wsUrl),
        pingInterval: debugPingInterval,
      );
      await _channel!.ready;
    } catch (e) {
      _state = SocketState.disconnected;
      _onDisconnected(e);
      return;
    }

    // The channel may have been disposed while we were awaiting ready
    // (e.g. provider rebuild triggered dispose() concurrently).
    if (_channel == null) {
      _state = SocketState.disconnected;
      return;
    }

    _state = SocketState.authenticating;
    _authCompleter = Completer<void>();

    _subscription = _channel!.stream.listen(
      _handleRawMessage,
      onError: (Object error) {
        _failAuth(error);
        _resetConnection();
        _onDisconnected(error);
      },
      onDone: () {
        _failAuth(null);
        _resetConnection();
        _onDisconnected(null);
      },
    );

    // A token is presented up front; the relay's challenge is not needed.
    if (accessTokens != null) unawaited(_startTokenAuth());

    // Wait for auth to complete (or timeout).
    _authTimeout = Timer(const Duration(seconds: 8), () {
      if (_authCompleter != null && !_authCompleter!.isCompleted) {
        _authCompleter!.completeError(
          TimeoutException('Relay auth timed out after 8s'),
        );
      }
    });

    try {
      await _authCompleter!.future;
      _authTimeout?.cancel();
      _state = SocketState.connected;
      // A rotation that arrived during the first AUTH goes out now.
      _sendQueuedReauth();
      _onConnected();
    } catch (e) {
      _authTimeout?.cancel();
      await disconnect();
      _onDisconnected(e);
    }
  }

  /// Present a rotated token on the live connection (same-principal re-AUTH
  /// keeps every subscription). While the first AUTH is still in flight the
  /// token is queued and sent after AUTH OK. No-op without access tokens or
  /// a connection attempt.
  void reauthenticate(String token) {
    if (accessTokens == null || _state == SocketState.disconnected) return;
    if (_state != SocketState.connected || _pendingAuthToken != null) {
      _queuedReauthToken = token;
      return;
    }
    _tokenRefreshUsed = false;
    _sendTokenAuth(token);
  }

  /// Send a raw JSON array over the websocket.
  void send(List<dynamic> payload) {
    _channel?.sink.add(jsonEncode(payload));
  }

  /// Gracefully close the connection.
  Future<void> disconnect() async {
    _resetConnection();
    final channel = _channel;
    _channel = null;
    if (channel != null) {
      await channel.sink.close();
    }
  }

  void dispose() {
    _resetConnection();
    _channel?.sink.close();
    _channel = null;
  }

  void _resetConnection() {
    _state = SocketState.disconnected;
    _subscription?.cancel();
    _subscription = null;
    _authTimeout?.cancel();
    _authTimeout = null;
    _pendingAuthEventId = null;
    _epoch++;
    _tokenAuthStarted = false;
    _pendingAuthToken = null;
    _queuedReauthToken = null;
    _boundToken = null;
    _tokenRefreshUsed = false;
  }

  Future<void> _startTokenAuth() async {
    final tokens = accessTokens;
    if (tokens == null || _tokenAuthStarted) return;
    _tokenAuthStarted = true;
    final epoch = _epoch;
    final String? token;
    try {
      token = await tokens.fresh();
    } catch (error) {
      if (epoch == _epoch) _failAuth(error);
      return;
    }
    if (epoch != _epoch) return;
    if (token == null) {
      _failAuth(const RelayTokenUnavailableException());
      return;
    }
    _sendTokenAuth(token);
  }

  void _sendTokenAuth(String token) {
    _pendingAuthToken = token;
    send([
      'AUTH',
      {'token': token},
    ]);
  }

  void _handleTokenAuthOk(bool accepted, String message) {
    final token = _pendingAuthToken;
    if (token == null) return; // Unsolicited; nothing is waiting on it.
    _pendingAuthToken = null;
    final initial = _state == SocketState.authenticating;
    if (accepted) {
      _tokenRefreshUsed = false;
      _boundToken = token;
      if (initial) {
        // [connect] sends any queued rotation once the state is connected.
        if (_authCompleter != null && !_authCompleter!.isCompleted) {
          _authCompleter!.complete();
        }
        return;
      }
      _sendQueuedReauth();
      return;
    }
    final refreshable = isRefreshableTokenRejection(message);
    if (initial) {
      // The relay marks the connection failed after a rejected first AUTH
      // and refuses every later AUTH on it (ws.rs), so a refreshed token can
      // only be presented on a new socket.
      if (refreshable && refreshRejectedToken) {
        unawaited(_refreshForReconnect(token));
      } else {
        _failAuth(classifyRelayAuthFailure(message));
      }
      return;
    }
    if (refreshable && !_tokenRefreshUsed) {
      _tokenRefreshUsed = true;
      unawaited(_refreshAndResend(token));
      return;
    }
    // A failed live re-AUTH leaves the previous binding in force; the relay
    // closes the socket at that token's deadline and reconnect re-AUTHs.
    debugPrint('[RelaySocket] token re-AUTH rejected: $message');
    _sendQueuedReauth();
  }

  void _sendQueuedReauth() {
    final queued = _queuedReauthToken;
    _queuedReauthToken = null;
    if (queued != null && queued != _boundToken) reauthenticate(queued);
  }

  /// First AUTH was rejected as refreshable: refresh once, then fail this
  /// socket so the session reconnects with the new token.
  Future<void> _refreshForReconnect(String rejected) async {
    final epoch = _epoch;
    String? next;
    Object? failure;
    try {
      next = await accessTokens?.afterExpired(rejected);
    } catch (error) {
      failure = error;
    }
    if (epoch != _epoch) return;
    if (next == null) {
      // A thrown refresh is transient (session backoff); a null one means
      // the session cannot issue a token (parks until it does).
      _failAuth(failure ?? const RelayTokenUnavailableException());
      return;
    }
    _failAuth(const RelayTokenRefreshedException());
  }

  /// A live re-AUTH was rejected: the previous binding still holds, so one
  /// refreshed token may be presented on the same socket.
  Future<void> _refreshAndResend(String rejected) async {
    final epoch = _epoch;
    String? next;
    try {
      next = await accessTokens?.afterExpired(rejected);
    } catch (error) {
      debugPrint('[RelaySocket] token refresh failed: $error');
    }
    if (epoch != _epoch || next == null) return;
    if (_pendingAuthToken != null) {
      _queuedReauthToken = next;
      return;
    }
    _sendTokenAuth(next);
  }

  void _failAuth(Object? error) {
    if (_authCompleter != null && !_authCompleter!.isCompleted) {
      _authCompleter!.completeError(error ?? Exception('Connection closed'));
    }
  }

  void _handleRawMessage(dynamic raw) {
    final String text;
    if (raw is String) {
      text = raw;
    } else {
      return; // Binary frames are not part of the Nostr protocol.
    }

    final List<dynamic> data;
    try {
      data = jsonDecode(text) as List<dynamic>;
    } catch (_) {
      return; // Malformed JSON.
    }

    if (data.isEmpty) return;
    final type = data[0] as String;

    switch (type) {
      case 'AUTH':
        _handleAuthChallenge(data);
      case 'OK':
        _handleOk(data);
      default:
        // Pass EVENT, EOSE, NOTICE, etc. upstream.
        _onMessage(data);
    }
  }

  /// Handle the relay's AUTH challenge: sign a kind:22242 event and respond.
  void _handleAuthChallenge(List<dynamic> data) {
    if (data.length < 2) return;
    // Token communities never answer with NIP-42 (plan §3.4).
    if (accessTokens != null) {
      unawaited(_startTokenAuth());
      return;
    }
    final challenge = data[1] as String;

    if (_nsec == null) {
      _failAuth(Exception('No nsec available for NIP-42 auth'));
      return;
    }

    try {
      // Decode bech32 nsec to hex private key.
      final privkeyHex = nostr.Nip19.decode(payload: _nsec).data;
      if (privkeyHex.isEmpty) {
        _failAuth(Exception('Invalid nsec'));
        return;
      }

      // Build the auth tags.
      final tags = <List<String>>[
        ['relay', _wsUrl],
        ['challenge', challenge],
      ];

      // Create and sign the kind:22242 AUTH event.
      final event = nostr.Event.from(
        kind: EventKind.auth,
        content: '',
        tags: tags,
        secretKey: privkeyHex,
      );

      _pendingAuthEventId = event.id;
      send(['AUTH', event.toMap()]);
    } catch (e) {
      _failAuth(e);
    }
  }

  @visibleForTesting
  void debugHandleOkForTest(List<dynamic> data) => _onMessage(data);

  /// Handle OK frames. During auth, complete the auth flow.
  void _handleOk(List<dynamic> data) {
    if (data.length < 3) return;
    final eventId = data[1] as String;
    final accepted = data[2] as bool;

    if (accessTokens != null && eventId == 'auth') {
      _handleTokenAuthOk(
        accepted,
        data.length > 3 ? data[3] as String : 'Auth rejected by relay',
      );
      return;
    }

    // Check if this OK is for our pending AUTH event.
    if (_pendingAuthEventId != null && eventId == _pendingAuthEventId) {
      _pendingAuthEventId = null;
      if (accepted) {
        if (_authCompleter != null && !_authCompleter!.isCompleted) {
          _authCompleter!.complete();
        }
      } else {
        final message = data.length > 3
            ? data[3] as String
            : 'Auth rejected by relay';
        _failAuth(classifyRelayAuthFailure(message));
      }
      return;
    }

    // Pass non-auth OK frames upstream for pending event tracking.
    _onMessage(data);
  }
}
