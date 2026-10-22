import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/device_key/device_key.dart';
import 'package:kuzz/shared/device_key/device_proof.dart';
import 'package:kuzz/shared/relay/relay_socket.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../helpers/software_device_key.dart';

/// NIP-42 AUTH from the phone carries `["device", pk, sig]` over the whole
/// event (device-bound keys plan §2.1), and a `restricted: signed_out`
/// refusal surfaces as such.
void main() {
  late DeviceKey previous;
  late SoftwareDeviceKey device;

  setUp(() {
    previous = DeviceKey.instance;
    device = SoftwareDeviceKey(List.filled(32, 4));
    DeviceKey.instance = device;
  });
  tearDown(() => DeviceKey.instance = previous);

  test('AUTH is device-tagged; signed_out is reported', () async {
    final received = Completer<Map<String, dynamic>>();
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(() => server.close(force: true));
    server.listen((request) async {
      final ws = await WebSocketTransformer.upgrade(request);
      ws.add(jsonEncode(['AUTH', 'challenge-1']));
      ws.listen((raw) {
        final frame = jsonDecode(raw as String) as List<dynamic>;
        if (frame.first != 'AUTH') return;
        final event = frame[1] as Map<String, dynamic>;
        received.complete(event);
        ws.add(jsonEncode(['OK', event['id'], false, relaySignedOutReason]));
      });
    });

    final disconnected = Completer<Object?>();
    final keys = nostr.Keys.generate();
    final socket = RelaySocket(
      wsUrl: 'ws://127.0.0.1:${server.port}',
      nsec: keys.nsec,
      onMessage: (_) {},
      onConnected: () {},
      onDisconnected: (error) {
        if (!disconnected.isCompleted) disconnected.complete(error);
      },
    );
    addTearDown(socket.dispose);
    unawaited(socket.connect());

    final event = await received.future.timeout(const Duration(seconds: 5));
    final parsed = nostr.Event.fromMap(event);
    expect(parsed.kind, 22242);
    expect(parsed.pubkey, keys.public);
    final tags = parsed.tags;
    expect(tags.where((tag) => tag.first == deviceTagName), hasLength(1));
    final tag = tags.last;
    expect(tag[0], deviceTagName);
    expect(tag[1], hexEncode(device.publicKey));
    final message = eventProofMessage(
      pubkey: parsed.pubkey,
      createdAt: parsed.createdAt,
      kind: parsed.kind,
      tags: tags,
      content: parsed.content,
    );
    expect(
      verifyDeviceSignature(
        device.publicKey,
        utf8.encode(message),
        hexDecode(tag[2]),
      ),
      isTrue,
    );

    final error = await disconnected.future.timeout(const Duration(seconds: 5));
    expect(isRelaySignedOut(error), isTrue);
  });

  test('a slow signature for a closed socket is never sent', () async {
    // Connection n challenges with `c<n>` and records the AUTH it gets.
    final auths = <int, List<String>>{};
    final opened = <int>[];
    var connections = 0;
    final secondAuth = Completer<void>();
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(() => server.close(force: true));
    server.listen((request) async {
      final ws = await WebSocketTransformer.upgrade(request);
      final n = ++connections;
      opened.add(n);
      auths[n] = [];
      ws.add(jsonEncode(['AUTH', 'c$n']));
      ws.listen((raw) {
        final frame = jsonDecode(raw as String) as List<dynamic>;
        if (frame.first != 'AUTH') return;
        final event = nostr.Event.fromMap(frame[1] as Map<String, dynamic>);
        auths[n]!.add(
          event.tags.firstWhere((tag) => tag.first == 'challenge')[1],
        );
        if (n == 2 && !secondAuth.isCompleted) secondAuth.complete();
      });
    });

    final socket = RelaySocket(
      wsUrl: 'ws://127.0.0.1:${server.port}',
      nsec: nostr.Keys.generate().nsec,
      onMessage: (_) {},
      onConnected: () {},
      onDisconnected: (_) {},
    );
    addTearDown(socket.dispose);

    // The first challenge's signature is stuck in secure hardware...
    final firstHold = Completer<void>();
    device.hold = firstHold;
    await socket.connect();
    await _until(() => opened.length == 1);
    // Let the challenge arrive and the signature start.
    await Future<void>.delayed(const Duration(milliseconds: 200));

    // ...while the socket is replaced and the new one answers promptly.
    await socket.disconnect();
    device.hold = null;
    await socket.connect();
    await secondAuth.future.timeout(const Duration(seconds: 5));

    // The old signature finishes late: it must not reach the new socket.
    firstHold.complete();
    await Future<void>.delayed(const Duration(milliseconds: 200));
    expect(auths[1], isEmpty);
    expect(auths[2], ['c2']);
  });
}

Future<void> _until(bool Function() condition) async {
  for (var i = 0; i < 100 && !condition(); i++) {
    await Future<void>.delayed(const Duration(milliseconds: 20));
  }
}
