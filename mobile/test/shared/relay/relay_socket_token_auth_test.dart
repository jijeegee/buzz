import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:buzz/shared/relay/relay_access_tokens.dart';
import 'package:buzz/shared/relay/relay_socket.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_relay_access_tokens.dart';

/// Loopback relay that records every client frame and lets the test script
/// replies (plan §3.4 token AUTH).
class _ScriptedRelay {
  _ScriptedRelay._(this._server);

  final HttpServer _server;
  final List<List<dynamic>> frames = [];
  final StreamController<List<dynamic>> _frames = StreamController.broadcast();
  WebSocket? _client;
  int connections = 0;

  /// Reply for each client AUTH frame; defaults to accepting.
  List<dynamic> Function(Map<String, dynamic> auth) onAuth = (_) => [
    'OK',
    'auth',
    true,
    '',
  ];
  bool sendChallenge = true;

  static Future<_ScriptedRelay> start() async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final relay = _ScriptedRelay._(server);
    server.transform(WebSocketTransformer()).listen((ws) {
      relay.connections++;
      relay._client = ws;
      if (relay.sendChallenge) ws.add(jsonEncode(['AUTH', 'challenge-1']));
      // Mirrors ws.rs: a rejected FIRST AUTH marks the connection Failed and
      // every later AUTH on it is refused; a rejected live re-AUTH keeps the
      // previous binding.
      var authenticated = false;
      var failed = false;
      ws.listen((raw) {
        final frame = jsonDecode(raw as String) as List<dynamic>;
        relay.frames.add(frame);
        relay._frames.add(frame);
        if (frame[0] == 'AUTH' && frame[1] is Map) {
          if (failed) {
            ws.add(jsonEncode(_reject('authentication already failed')));
            return;
          }
          final reply = relay.onAuth(
            Map<String, dynamic>.from(frame[1] as Map),
          );
          if (reply.isEmpty) return;
          if (reply[2] == true) {
            authenticated = true;
          } else if (!authenticated) {
            failed = true;
          }
          ws.add(jsonEncode(reply));
        }
      }, onError: (_) {});
    });
    return relay;
  }

  String get url => 'ws://127.0.0.1:${_server.port}';

  List<List<dynamic>> get authFrames =>
      frames.where((f) => f[0] == 'AUTH').toList();

  Future<List<dynamic>> nextFrame() => _frames.stream.first;

  void push(List<dynamic> frame) => _client?.add(jsonEncode(frame));

  Future<void> close() async {
    await _client?.close();
    await _server.close(force: true);
  }
}

List<dynamic> _reject(String code) => [
  'OK',
  'auth',
  false,
  'auth-required: $code',
];

void main() {
  late _ScriptedRelay relay;

  setUp(() async {
    relay = await _ScriptedRelay.start();
  });

  tearDown(() async {
    await relay.close();
  });

  RelaySocket socketFor(
    FakeRelayAccessTokens tokens, {
    required Completer<void> connected,
    required Completer<Object?> disconnected,
  }) {
    return RelaySocket(
      wsUrl: relay.url,
      nsec: null,
      onMessage: (_) {},
      onConnected: () {
        if (!connected.isCompleted) connected.complete();
      },
      onDisconnected: (error) {
        if (!disconnected.isCompleted) disconnected.complete(error);
      },
    )..accessTokens = tokens;
  }

  test('answers with the token AUTH frame, never NIP-42', () async {
    final tokens = FakeRelayAccessTokens(token: 'bzs_1');
    final connected = Completer<void>();
    final socket = socketFor(
      tokens,
      connected: connected,
      disconnected: Completer<Object?>(),
    );

    await socket.connect();
    await connected.future.timeout(const Duration(seconds: 2));

    expect(relay.authFrames, [
      [
        'AUTH',
        {'token': 'bzs_1'},
      ],
    ]);
    expect(socket.state, SocketState.connected);
    socket.dispose();
  });

  test(
    'first-AUTH token_expired refreshes, then fails retryable: the relay has '
    'failed this socket, so the new token goes on a NEW socket',
    () async {
      final tokens = FakeRelayAccessTokens(
        token: 'bzs_1',
        rotations: ['bzs_2'],
      );
      relay.onAuth = (auth) => auth['token'] == 'bzs_1'
          ? _reject('token_expired')
          : ['OK', 'auth', true, ''];
      final disconnected = Completer<Object?>();
      final first = socketFor(
        tokens,
        connected: Completer<void>(),
        disconnected: disconnected,
      );

      await first.connect();
      final error = await disconnected.future.timeout(
        const Duration(seconds: 2),
      );

      expect(error, isA<RelayTokenRefreshedException>());
      expect(tokens.expiredCalls, ['bzs_1']);
      // Never re-AUTHs on the failed connection.
      expect(relay.authFrames.map((f) => (f[1] as Map)['token']), ['bzs_1']);

      final connected = Completer<void>();
      final second = socketFor(
        tokens,
        connected: connected,
        disconnected: Completer<Object?>(),
      )..refreshRejectedToken = false;
      await second.connect();
      await connected.future.timeout(const Duration(seconds: 2));

      expect(relay.connections, 2);
      expect(relay.authFrames.map((f) => (f[1] as Map)['token']), [
        'bzs_1',
        'bzs_2',
      ]);
      expect(second.state, SocketState.connected);
      second.dispose();
    },
  );

  test('a rejection on the retry socket is terminal (bounded)', () async {
    final tokens = FakeRelayAccessTokens(
      token: 'bzs_1',
      rotations: ['bzs_2', 'bzs_3'],
    );
    relay.onAuth = (_) => _reject('token_expired');
    final disconnected = Completer<Object?>();
    final socket = socketFor(
      tokens,
      connected: Completer<void>(),
      disconnected: disconnected,
    )..refreshRejectedToken = false;

    await socket.connect();
    final error = await disconnected.future.timeout(const Duration(seconds: 2));

    expect(error, isA<RelayAuthRejectedException>());
    expect(tokens.expiredCalls, isEmpty);
    expect(relay.authFrames, hasLength(1));
  });

  test(
    'a live re-AUTH rejection refreshes and retries on the same socket',
    () async {
      final tokens = FakeRelayAccessTokens(
        token: 'bzs_1',
        rotations: ['bzs_3'],
      );
      relay.onAuth = (auth) => auth['token'] == 'bzs_2'
          ? _reject('token_expired')
          : ['OK', 'auth', true, ''];
      final connected = Completer<void>();
      final socket = socketFor(
        tokens,
        connected: connected,
        disconnected: Completer<Object?>(),
      );
      await socket.connect();
      await connected.future.timeout(const Duration(seconds: 2));

      socket.reauthenticate('bzs_2');
      await Future<void>.delayed(const Duration(milliseconds: 100));

      expect(tokens.expiredCalls, ['bzs_2']);
      expect(relay.authFrames.map((f) => (f[1] as Map)['token']), [
        'bzs_1',
        'bzs_2',
        'bzs_3',
      ]);
      expect(relay.connections, 1);
      expect(socket.state, SocketState.connected);
      socket.dispose();
    },
  );

  test('a rotation during the first AUTH is sent after AUTH OK', () async {
    final tokens = FakeRelayAccessTokens(token: 'bzs_1');
    relay.onAuth = (auth) =>
        auth['token'] == 'bzs_1' ? const [] : ['OK', 'auth', true, ''];
    final connected = Completer<void>();
    final socket = socketFor(
      tokens,
      connected: connected,
      disconnected: Completer<Object?>(),
    );

    unawaited(socket.connect());
    expect(await relay.nextFrame().timeout(const Duration(seconds: 2)), [
      'AUTH',
      {'token': 'bzs_1'},
    ]);
    expect(socket.state, SocketState.authenticating);
    socket.reauthenticate('bzs_2');
    final next = relay.nextFrame();
    relay.push(['OK', 'auth', true, '']);
    await connected.future.timeout(const Duration(seconds: 2));

    expect(await next.timeout(const Duration(seconds: 2)), [
      'AUTH',
      {'token': 'bzs_2'},
    ]);
    socket.dispose();
  });

  test('a failed refresh surfaces RelayTokenUnavailableException', () async {
    final tokens = FakeRelayAccessTokens(token: 'bzs_1');
    relay.onAuth = (_) => _reject('token_revoked');
    final disconnected = Completer<Object?>();
    final socket = socketFor(
      tokens,
      connected: Completer<void>(),
      disconnected: disconnected,
    );

    await socket.connect();
    final error = await disconnected.future.timeout(const Duration(seconds: 2));

    expect(error, isA<RelayTokenUnavailableException>());
    expect(tokens.expiredCalls, ['bzs_1']);
  });

  test('no token: fails without sending AUTH', () async {
    final tokens = FakeRelayAccessTokens(token: null);
    final disconnected = Completer<Object?>();
    final socket = socketFor(
      tokens,
      connected: Completer<void>(),
      disconnected: disconnected,
    );

    await socket.connect();
    final error = await disconnected.future.timeout(const Duration(seconds: 2));

    expect(error, isA<RelayTokenUnavailableException>());
    expect(relay.authFrames, isEmpty);
  });

  test('transient auth outage reconnects instead of terminating', () async {
    final tokens = FakeRelayAccessTokens(token: 'bzs_1');
    relay.onAuth = (_) => _reject('unavailable');
    final disconnected = Completer<Object?>();
    final socket = socketFor(
      tokens,
      connected: Completer<void>(),
      disconnected: disconnected,
    );

    await socket.connect();
    final error = await disconnected.future.timeout(const Duration(seconds: 2));

    expect(error, isNot(isA<RelayAuthRejectedException>()));
    expect(error, isNot(isA<RelayTokenUnavailableException>()));
    expect(tokens.expiredCalls, isEmpty);
  });

  test('reauthenticate sends the rotated token on the live socket', () async {
    final tokens = FakeRelayAccessTokens(token: 'bzs_1');
    final connected = Completer<void>();
    final socket = socketFor(
      tokens,
      connected: connected,
      disconnected: Completer<Object?>(),
    );
    await socket.connect();
    await connected.future.timeout(const Duration(seconds: 2));

    final next = relay.nextFrame();
    socket.reauthenticate('bzs_2');
    expect(await next.timeout(const Duration(seconds: 2)), [
      'AUTH',
      {'token': 'bzs_2'},
    ]);
    // The OK auth reply for a re-AUTH is consumed, not leaked upstream.
    await Future<void>.delayed(const Duration(milliseconds: 50));
    expect(relay.connections, 1);
    expect(socket.state, SocketState.connected);
    socket.dispose();
  });

  test('legacy (no access tokens) still fails NIP-42 without nsec', () async {
    final disconnected = Completer<Object?>();
    final socket = RelaySocket(
      wsUrl: relay.url,
      nsec: null,
      onMessage: (_) {},
      onConnected: () {},
      onDisconnected: (error) {
        if (!disconnected.isCompleted) disconnected.complete(error);
      },
    );
    await socket.connect();
    final error = await disconnected.future.timeout(const Duration(seconds: 2));
    expect(error.toString(), contains('No nsec'));
    expect(relay.authFrames, isEmpty);
  });
}
