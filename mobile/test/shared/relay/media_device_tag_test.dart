import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;
import 'package:kuzz/shared/device_key/device_key.dart';
import 'package:kuzz/shared/device_key/device_proof.dart';
import 'package:kuzz/shared/relay/media_auth.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../helpers/software_device_key.dart';

Map<String, dynamic> _event(String? authorization) {
  final encoded = authorization!.substring('Nostr '.length);
  return jsonDecode(utf8.decode(base64Url.decode(base64Url.normalize(encoded))))
      as Map<String, dynamic>;
}

List<List<String>> _tags(Map<String, dynamic> event) => [
  for (final tag in event['tags'] as List<dynamic>)
    (tag as List<dynamic>).cast<String>(),
];

/// Blossom read tokens carry the device tag; because secure hardware signs
/// slowly, one tagged token is reused for a short while and re-signed in the
/// background.
void main() {
  late DeviceKey previous;
  late SoftwareDeviceKey device;
  late DateTime now;
  final url = 'https://relay.example/media/${'a' * 64}.jpg';

  setUp(() {
    previous = DeviceKey.instance;
    device = SoftwareDeviceKey(List.filled(32, 8));
    DeviceKey.instance = device;
    now = DateTime.now();
  });
  tearDown(() => DeviceKey.instance = previous);

  MediaGetAuthService service() => MediaGetAuthService(
    baseUrl: 'https://relay.example',
    nsec: nostr.Keys.generate().nsec,
    now: () => now,
  );

  bool tagVerifies(Map<String, dynamic> event) {
    final tags = _tags(event);
    final tag = tags.singleWhere((tag) => tag.first == deviceTagName);
    return tag[1] == hexEncode(device.publicKey) &&
        verifyDeviceSignature(
          device.publicKey,
          utf8.encode(
            eventProofMessage(
              pubkey: event['pubkey'] as String,
              createdAt: event['created_at'] as int,
              kind: event['kind'] as int,
              tags: tags,
              content: event['content'] as String,
            ),
          ),
          hexDecode(tag[2]),
        );
  }

  test('async fetches wait for a tagged token and reuse it', () async {
    final auth = service();
    final sent = <http.Request>[];
    final client = http_testing.MockClient((request) async {
      sent.add(request);
      return http.Response('', 200);
    });
    await auth.get(client, url);
    await auth.get(client, url);
    expect(sent, hasLength(2));
    for (final request in sent) {
      expect(tagVerifies(_event(request.headers['Authorization'])), isTrue);
    }
    expect(device.signCount, 1);
  });

  test('sync headers fall back untagged, then use the tagged token', () async {
    final auth = service();
    final first = _event(auth.headersFor(url)['Authorization']);
    expect(_tags(first).where((tag) => tag.first == deviceTagName), isEmpty);
    await pumpEventQueue();
    final second = _event(auth.headersFor(url)['Authorization']);
    expect(tagVerifies(second), isTrue);

    // Due for re-signing: the current token is still sent meanwhile.
    now = now.add(const Duration(seconds: 30));
    expect(auth.headersFor(url)['Authorization'], isNotNull);
    await pumpEventQueue();
    expect(device.signCount, 2);

    // Past its use window with no fresh token: untagged again.
    device.failSigning = true;
    now = now.add(const Duration(seconds: 60));
    final late = _event(auth.headersFor(url)['Authorization']);
    expect(_tags(late).where((tag) => tag.first == deviceTagName), isEmpty);
  });
}
