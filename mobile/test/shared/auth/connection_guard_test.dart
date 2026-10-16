import 'package:flutter_test/flutter_test.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:web_socket_channel/web_socket_channel.dart';

import 'package:kuzz/shared/auth/default_community.dart';
import 'package:kuzz/shared/huddle/huddle_auth.dart';
import 'package:kuzz/shared/huddle/huddle_transport.dart';
import 'package:kuzz/shared/relay/relay_socket.dart';
import 'package:kuzz/shared/relay/signed_event_relay.dart';

const _parentChannelId = '11111111-2222-4333-8444-555555555555';
const _ephemeralChannelId = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee';

/// The connection layer refuses any server other than the built-in one when
/// the socket would be created, whatever address its caller passed.
void main() {
  setUp(() {
    final previous = debugUnrestrictedServerForTests;
    // This build's real rule (prod, `https://jihobuzz.duckdns.org:8443`).
    debugUnrestrictedServerForTests = false;
    addTearDown(() => debugUnrestrictedServerForTests = previous);
  });

  test('allowConnection follows the built-in server rule', () {
    expect(allowConnection('test', 'wss://jihobuzz.duckdns.org:8443'), isTrue);
    expect(allowConnection('test', 'wss://other.example'), isFalse);
    expect(allowConnection('test', 'ws://jihobuzz.duckdns.org:8443'), isFalse);
    expect(allowConnection('test', 'not a url'), isFalse);
  });

  for (final url in ['wss://other.example', 'ws://jihobuzz.duckdns.org:8443']) {
    test('RelaySocket never opens $url', () async {
      Object? disconnectError;
      var connected = false;
      final socket = RelaySocket(
        wsUrl: url,
        nsec: null,
        onMessage: (_) {},
        onConnected: () => connected = true,
        onDisconnected: (error) => disconnectError = error,
      );
      addTearDown(socket.dispose);

      await socket.connect();

      expect(connected, isFalse);
      expect(socket.state, SocketState.disconnected);
      expect(
        disconnectError,
        isA<StateError>().having(
          (error) => error.message,
          'message',
          differentServerMessage,
        ),
      );
    });
  }

  test('submitSignedEventOnce refuses another server before signing', () {
    expect(
      submitSignedEventOnce(
        wsUrl: 'wss://other.example',
        nsec: nostr.Keys.generate().nsec,
        kind: 1,
        content: '',
        tags: const [],
      ),
      throwsA(isA<StateError>()),
    );
  });

  test('HuddleTransport never creates a socket for another server', () async {
    var created = 0;
    final transport = HuddleTransport(
      parameters: HuddleConnectionParameters(
        relayWebSocketUrl: 'wss://other.example',
        nsec: nostr.Keys.generate().nsec,
        parentChannelId: _parentChannelId,
        ephemeralChannelId: _ephemeralChannelId,
      ),
      channelFactory: (uri) {
        created++;
        return WebSocketChannel.connect(uri);
      },
    );
    addTearDown(transport.dispose);

    await expectLater(
      transport.connect(),
      throwsA(
        isA<HuddleTransportError>()
            .having(
              (error) => error.code,
              'code',
              HuddleTransportErrorCode.connectionFailed,
            )
            .having(
              (error) => error.message,
              'message',
              differentServerMessage,
            ),
      ),
    );
    expect(created, 0);
    expect(transport.state.phase, HuddleTransportPhase.failed);
  });
}
