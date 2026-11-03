import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/device_key/device_key.dart';
import 'package:kuzz/shared/device_key/device_proof.dart';
import 'package:kuzz/shared/relay/nip98.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../helpers/software_device_key.dart';

/// The NIP-98 header the app and the Android background push handler send
/// with `/query`: device-tagged when the phone has a device key, and unique
/// per call through its `nonce`.
void main() {
  late DeviceKey previous;
  setUp(() => previous = DeviceKey.instance);
  tearDown(() => DeviceKey.instance = previous);

  final keys = nostr.Keys.generate();
  final nsec = keys.nsec;

  Map<String, dynamic> decode(String header) {
    expect(header, startsWith('Nostr '));
    return jsonDecode(utf8.decode(base64.decode(header.substring(6))))
        as Map<String, dynamic>;
  }

  List<String>? tag(Map<String, dynamic> event, String name) => [
    for (final t in event['tags'] as List)
      if ((t as List).first == name) t.cast<String>(),
  ].firstOrNull;

  test('carries u, method, payload, a nonce and the device tag', () async {
    final device = SoftwareDeviceKey(List.filled(32, 7));
    DeviceKey.instance = device;
    final event = decode(
      await buildNip98AuthHeader(
        method: 'post',
        url: 'https://relay.example/query',
        bodyBytes: utf8.encode('[]'),
        nsec: nsec,
      ),
    );
    expect(event['kind'], 27235);
    expect(event['pubkey'], keys.public);
    expect(tag(event, 'u')![1], 'https://relay.example/query');
    expect(tag(event, 'method')![1], 'POST');
    expect(tag(event, 'payload')![1], hasLength(64));
    expect(tag(event, 'nonce')![1], isNotEmpty);
    expect(tag(event, 'device')![1], hexEncode(device.publicKey));
    expect(device.signCount, 1);
  });

  test('two headers for the same request differ', () async {
    DeviceKey.instance = SoftwareDeviceKey(List.filled(32, 8));
    Future<Map<String, dynamic>> one() async => decode(
      await buildNip98AuthHeader(
        method: 'POST',
        url: 'https://relay.example/query',
        bodyBytes: const [],
        nsec: nsec,
      ),
    );
    final a = await one();
    final b = await one();
    expect(a['id'], isNot(b['id']));
    expect(tag(a, 'nonce')![1], isNot(tag(b, 'nonce')![1]));
  });

  test('refuses to sign without a key', () {
    expect(
      () => buildNip98AuthHeader(
        method: 'POST',
        url: 'https://relay.example/query',
        bodyBytes: const [],
        nsec: null,
      ),
      throwsException,
    );
  });
}
