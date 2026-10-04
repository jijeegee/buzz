import 'dart:async';
import 'dart:convert';

import 'package:buzz/shared/auth/token/token_session.dart';
import 'package:buzz/shared/huddle/huddle_auth.dart';
import 'package:buzz/shared/huddle/huddle_transport.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

import '../relay/fake_relay_access_tokens.dart';

const _parentChannelId = '11111111-2222-4333-8444-555555555555';
const _ephemeralChannelId = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee';

void main() {
  group('HuddleConnectionParameters on a token community', () {
    test('accepts access tokens without an nsec', () {
      final parameters = _parameters(FakeRelayAccessTokens());
      expect(parameters.nsec, isNull);
      expect(parameters.accessTokens, isNotNull);
    });

    test('rejects a connection with neither an nsec nor tokens', () {
      expect(
        () => HuddleConnectionParameters(
          relayWebSocketUrl: 'wss://relay.example.com',
          parentChannelId: _parentChannelId,
          ephemeralChannelId: _ephemeralChannelId,
        ),
        throwsArgumentError,
      );
    });
  });

  group('HuddleTransport token auth', () {
    test('answers the challenge with a fresh bearer token, no event', () async {
      final tokens = FakeRelayAccessTokens(token: 'bzs_1');
      final channel = _FakeChannel();
      final transport = _transport(channel, tokens);
      addTearDown(transport.dispose);

      await _admit(transport, channel);

      final auth = jsonDecode(channel.sink.sent.single as String) as Map;
      expect(auth, {
        'type': 'auth',
        'token': 'bzs_1',
        'parent_channel_id': _parentChannelId,
        'protocol_version': 2,
      });
      expect(tokens.freshCalls, 1);
      expect(transport.state.phase, HuddleTransportPhase.connected);
    });

    test('fails authentication when no token is available', () async {
      final tokens = FakeRelayAccessTokens(token: null);
      final channel = _FakeChannel();
      final transport = _transport(channel, tokens);
      addTearDown(transport.dispose);

      final connect = transport.connect();
      await _waitForPhase(transport, HuddleTransportPhase.awaitingChallenge);
      channel.emitText(jsonEncode({'type': 'challenge', 'challenge': 'c'}));

      await expectLater(
        connect,
        throwsA(
          isA<HuddleTransportError>().having(
            (e) => e.code,
            'code',
            HuddleTransportErrorCode.authenticationFailed,
          ),
        ),
      );
      expect(channel.sink.sent, isEmpty);
      expect(channel.sink.closed, isTrue);
    });

    test('re-authenticates on rotation over the same socket', () async {
      final tokens = FakeRelayAccessTokens(token: 'bzs_1');
      final channel = _FakeChannel();
      final transport = _transport(channel, tokens);
      addTearDown(transport.dispose);
      final issues = <HuddleTransportError>[];
      transport.issues.listen(issues.add);
      await _admit(transport, channel);
      channel.sink.sent.clear();

      tokens.rotate('bzs_2');

      expect(channel.sink.sent.map((m) => jsonDecode(m as String)), [
        {'type': 'auth', 'token': 'bzs_2'},
      ]);
      channel.emitText(jsonEncode({'type': 'auth_ok'}));
      await pumpEventQueue();
      expect(issues, isEmpty);
      expect(transport.state.phase, HuddleTransportPhase.connected);

      // The same token again is not resent.
      tokens.rotate('bzs_2');
      expect(channel.sink.sent, hasLength(1));
    });

    test('keeps re-auth single-flight and sends the latest token after '
        'auth_ok', () async {
      final tokens = FakeRelayAccessTokens(token: 'bzs_1');
      final channel = _FakeChannel();
      final transport = _transport(channel, tokens);
      addTearDown(transport.dispose);
      await _admit(transport, channel);
      channel.sink.sent.clear();

      tokens.rotate('bzs_2');
      tokens.rotate('bzs_3');
      expect(channel.sink.sent, hasLength(1));

      channel.emitText(jsonEncode({'type': 'auth_ok'}));
      await pumpEventQueue();

      expect(channel.sink.sent.map((m) => jsonDecode(m as String)), [
        {'type': 'auth', 'token': 'bzs_2'},
        {'type': 'auth', 'token': 'bzs_3'},
      ]);
    });

    test('auth_error is a recoverable issue, not a teardown', () async {
      final tokens = FakeRelayAccessTokens(token: 'bzs_1');
      final channel = _FakeChannel();
      final transport = _transport(channel, tokens);
      addTearDown(transport.dispose);
      final issues = <HuddleTransportError>[];
      transport.issues.listen(issues.add);
      await _admit(transport, channel);

      tokens.rotate('bzs_2');
      channel.emitText(
        jsonEncode({'type': 'auth_error', 'message': 'auth-required: x'}),
      );
      await pumpEventQueue();

      expect(transport.state.phase, HuddleTransportPhase.connected);
      expect(issues.single.code, HuddleTransportErrorCode.authenticationFailed);
      expect(issues.single.message, 'auth-required: x');
    });

    test('signing out closes the socket and drops the listeners', () async {
      final tokens = FakeRelayAccessTokens(token: 'bzs_1');
      final channel = _FakeChannel();
      final transport = _transport(channel, tokens);
      addTearDown(transport.dispose);
      await _admit(transport, channel);
      expect(tokens.tokenListenerCount, 1);
      expect(tokens.stateListenerCount, 1);

      tokens.emitState(
        const TokenSessionState(status: TokenSessionStatus.signedOut),
      );

      expect(transport.state.phase, HuddleTransportPhase.failed);
      expect(
        transport.state.error?.code,
        HuddleTransportErrorCode.authenticationFailed,
      );
      expect(channel.sink.closed, isTrue);
      expect(tokens.tokenListenerCount, 0);
      expect(tokens.stateListenerCount, 0);
    });

    test('a first-AUTH token_expired refreshes once and admits on a NEW '
        'socket with the new token', () async {
      final tokens = FakeRelayAccessTokens(
        token: 'bzs_1',
        rotations: ['bzs_2'],
      );
      final channels = [_FakeChannel(), _FakeChannel()];
      final transport = _multiTransport(channels, tokens);
      addTearDown(transport.dispose);

      final phases = <HuddleTransportPhase>[];
      transport.states.listen((state) => phases.add(state.phase));

      final connect = transport.connect();
      await _waitForPhase(transport, HuddleTransportPhase.awaitingChallenge);
      channels[0].emitText(jsonEncode({'type': 'challenge', 'challenge': 'c'}));
      await _waitForPhase(transport, HuddleTransportPhase.authenticating);
      channels[0].emitText(
        jsonEncode({
          'type': 'error',
          'message': 'auth-required: token_expired',
        }),
      );
      await _waitForSent(
        channels[1],
        HuddleTransportPhase.awaitingChallenge,
        transport,
      );
      channels[1].emitText(jsonEncode({'type': 'challenge', 'challenge': 'd'}));
      await _waitForPhase(transport, HuddleTransportPhase.authenticating);
      await pumpEventQueue();
      channels[1].emitText(
        jsonEncode({'type': 'joined', 'pubkey': 'self', 'peer_index': 1}),
      );
      await connect;

      // Consumers (HuddleSessionNotifier) tear down on `failed`; the
      // refresh retry must not surface one.
      expect(phases, isNot(contains(HuddleTransportPhase.failed)));
      expect(tokens.expiredCalls, ['bzs_1']);
      expect(channels[0].sink.closed, isTrue);
      expect(
        (jsonDecode(channels[1].sink.sent.single as String) as Map)['token'],
        'bzs_2',
      );
      expect(transport.state.phase, HuddleTransportPhase.connected);
    });

    test('a second first-AUTH rejection is terminal (one refresh)', () async {
      final tokens = FakeRelayAccessTokens(
        token: 'bzs_1',
        rotations: ['bzs_2', 'bzs_3'],
      );
      final channels = [_FakeChannel(), _FakeChannel(), _FakeChannel()];
      final transport = _multiTransport(channels, tokens);
      addTearDown(transport.dispose);

      final connect = transport.connect();
      for (final channel in channels.take(2)) {
        await _waitForSent(
          channel,
          HuddleTransportPhase.awaitingChallenge,
          transport,
        );
        channel.emitText(jsonEncode({'type': 'challenge', 'challenge': 'c'}));
        await _waitForPhase(transport, HuddleTransportPhase.authenticating);
        await pumpEventQueue();
        channel.emitText(
          jsonEncode({
            'type': 'error',
            'message': 'auth-required: token_expired',
          }),
        );
      }

      await expectLater(
        connect,
        throwsA(
          isA<HuddleTransportError>().having(
            (e) => e.code,
            'code',
            HuddleTransportErrorCode.relayRejected,
          ),
        ),
      );
      expect(tokens.expiredCalls, ['bzs_1']);
      expect(channels[2].sink.sent, isEmpty);
    });

    test(
      'a first-AUTH rejection whose refresh fails asks to sign in',
      () async {
        final tokens = FakeRelayAccessTokens(token: 'bzs_1');
        final channels = [_FakeChannel(), _FakeChannel()];
        final transport = _multiTransport(channels, tokens);
        addTearDown(transport.dispose);

        final connect = transport.connect();
        await _waitForPhase(transport, HuddleTransportPhase.awaitingChallenge);
        channels[0].emitText(
          jsonEncode({'type': 'challenge', 'challenge': 'c'}),
        );
        await _waitForPhase(transport, HuddleTransportPhase.authenticating);
        channels[0].emitText(
          jsonEncode({
            'type': 'error',
            'message': 'auth-required: token_revoked',
          }),
        );

        await expectLater(
          connect,
          throwsA(
            isA<HuddleTransportError>().having(
              (e) => e.code,
              'code',
              HuddleTransportErrorCode.authenticationFailed,
            ),
          ),
        );
        expect(tokens.expiredCalls, ['bzs_1']);
        expect(channels[1].sink.sent, isEmpty);
      },
    );

    test('a non-token relay error is not refreshed', () async {
      final tokens = FakeRelayAccessTokens(token: 'bzs_1', rotations: ['x']);
      final channels = [_FakeChannel(), _FakeChannel()];
      final transport = _multiTransport(channels, tokens);
      addTearDown(transport.dispose);

      final connect = transport.connect();
      await _waitForPhase(transport, HuddleTransportPhase.awaitingChallenge);
      channels[0].emitText(jsonEncode({'type': 'challenge', 'challenge': 'c'}));
      await _waitForPhase(transport, HuddleTransportPhase.authenticating);
      channels[0].emitText(
        jsonEncode({
          'type': 'error',
          'message': 'restricted: not a channel member',
        }),
      );

      await expectLater(connect, throwsA(isA<HuddleTransportError>()));
      expect(tokens.expiredCalls, isEmpty);
    });

    test('disconnect drops the token listeners', () async {
      final tokens = FakeRelayAccessTokens(token: 'bzs_1');
      final channel = _FakeChannel();
      final transport = _transport(channel, tokens);
      addTearDown(transport.dispose);
      await _admit(transport, channel);

      await transport.disconnect();

      expect(tokens.tokenListenerCount, 0);
      expect(tokens.stateListenerCount, 0);
    });
  });
}

HuddleConnectionParameters _parameters(FakeRelayAccessTokens tokens) =>
    HuddleConnectionParameters(
      relayWebSocketUrl: 'wss://relay.example.com',
      accessTokens: tokens,
      parentChannelId: _parentChannelId,
      ephemeralChannelId: _ephemeralChannelId,
    );

HuddleTransport _transport(
  _FakeChannel channel,
  FakeRelayAccessTokens tokens,
) => HuddleTransport(
  parameters: _parameters(tokens),
  channelFactory: (_) => channel,
);

HuddleTransport _multiTransport(
  List<_FakeChannel> channels,
  FakeRelayAccessTokens tokens,
) {
  var next = 0;
  return HuddleTransport(
    parameters: _parameters(tokens),
    channelFactory: (_) => channels[next++],
  );
}

/// Waits until [channel] is the transport's live socket in [phase]: its
/// stream has a listener and the transport reached [phase].
Future<void> _waitForSent(
  _FakeChannel channel,
  HuddleTransportPhase phase,
  HuddleTransport transport,
) async {
  for (var attempt = 0; attempt < 100; attempt++) {
    if (channel.listened && transport.state.phase == phase) return;
    await Future<void>.delayed(const Duration(milliseconds: 1));
  }
  fail(
    'Channel never became live in ${phase.name}; '
    'was ${transport.state.phase}.',
  );
}

Future<void> _admit(HuddleTransport transport, _FakeChannel channel) async {
  final connect = transport.connect();
  await _waitForPhase(transport, HuddleTransportPhase.awaitingChallenge);
  channel.emitText(jsonEncode({'type': 'challenge', 'challenge': 'c'}));
  await _waitForPhase(transport, HuddleTransportPhase.authenticating);
  channel.emitText(
    jsonEncode({'type': 'joined', 'pubkey': 'self', 'peer_index': 1}),
  );
  await connect;
}

Future<void> _waitForPhase(
  HuddleTransport transport,
  HuddleTransportPhase phase,
) async {
  for (var attempt = 0; attempt < 100; attempt++) {
    if (transport.state.phase == phase) return;
    await Future<void>.delayed(const Duration(milliseconds: 1));
  }
  fail('Transport never reached ${phase.name}; was ${transport.state.phase}.');
}

final class _FakeChannel implements WebSocketChannel {
  final StreamController<dynamic> _controller = StreamController();
  final _FakeSink _sink = _FakeSink();

  void emitText(String value) => _controller.add(value);

  bool get listened => _controller.hasListener;

  @override
  Future<void> get ready => Future.value();

  @override
  Stream<dynamic> get stream => _controller.stream;

  @override
  _FakeSink get sink => _sink;

  @override
  int? get closeCode => null;

  @override
  String? get closeReason => null;

  @override
  String? get protocol => null;

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

final class _FakeSink implements WebSocketSink {
  final List<dynamic> sent = [];
  bool closed = false;

  @override
  void add(dynamic event) => sent.add(event);

  @override
  void addError(Object error, [StackTrace? stackTrace]) {}

  @override
  Future<void> addStream(Stream<dynamic> stream) async {
    await for (final event in stream) {
      sent.add(event);
    }
  }

  @override
  Future<void> close([int? closeCode, String? closeReason]) async {
    closed = true;
  }

  @override
  Future<void> get done => Future.value();
}
