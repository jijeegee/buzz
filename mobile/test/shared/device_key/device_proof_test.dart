import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/device_key/device_key.dart';
import 'package:kuzz/shared/device_key/device_proof.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../helpers/software_device_key.dart';

/// Shared with the relay (`kuzz-auth/src/device_proof.rs`), the CLI and the
/// desktop app: every client must produce these exact messages.
final Map<String, dynamic> _vectors =
    jsonDecode(
          File(
            '../crates/kuzz-auth/src/device_proof_vectors.json',
          ).readAsStringSync(),
        )
        as Map<String, dynamic>;

List<List<String>> _tags(Map<String, dynamic> event) => [
  for (final tag in event['tags'] as List<dynamic>)
    [for (final value in tag as List<dynamic>) value as String],
];

void main() {
  final device = SoftwareDeviceKey(
    hexDecode(_vectors['device_secret_hex'] as String),
  );

  group('shared device proof vectors', () {
    test('the device public key matches', () async {
      final info = await device.info();
      expect(info!.publicKeyHex, _vectors['device_pubkey_hex']);
      expect(info.kind, 'hardware');
    });

    test('the header proof message and signature match', () async {
      final header = _vectors['header_proof'] as Map<String, dynamic>;
      final message = headerProofMessage(
        purpose: header['purpose'] as String,
        binding: header['binding'] as String,
        tokenHash: hexDecode(header['token_sha256_hex'] as String),
        bodyHash: hexDecode(header['body_sha256_hex'] as String),
        ts: header['ts'] as int,
        nonce: header['nonce'] as String,
      );
      expect(message, header['message']);
      expect(
        verifyDeviceSignature(
          hexDecode(_vectors['device_pubkey_hex'] as String),
          utf8.encode(message),
          hexDecode(header['signature_hex'] as String),
        ),
        isTrue,
      );
      // RFC 6979 makes the signature deterministic: same bytes as the relay.
      final proof = await signHeaderProof(
        device: device,
        origin: 'https://relay.example',
        purpose: header['purpose'] as String,
        binding: header['binding'] as String,
        tokenHash: hexDecode(header['token_sha256_hex'] as String),
        bodyHash: hexDecode(header['body_sha256_hex'] as String),
        ts: header['ts'] as int,
        nonce: header['nonce'] as String,
      );
      expect(proof, header['header']);
    });

    test('event messages match, including JSON escaping', () {
      final vectors = _vectors['event_tags'] as List<dynamic>;
      expect(vectors, hasLength(2));
      for (final vector in vectors.cast<Map<String, dynamic>>()) {
        final event = vector['event'] as Map<String, dynamic>;
        final tags = _tags(event);
        final message = eventProofMessage(
          pubkey: event['pubkey'] as String,
          createdAt: event['created_at'] as int,
          kind: event['kind'] as int,
          tags: tags,
          content: event['content'] as String,
        );
        expect(message, vector['message']);
        final deviceTag = tags.singleWhere((tag) => tag.first == 'device');
        expect(
          verifyDeviceSignature(
            hexDecode(deviceTag[1]),
            utf8.encode(message),
            hexDecode(deviceTag[2]),
          ),
          isTrue,
        );
      }
      // The escaping vector covers quotes, controls, DEL, "/", Korean,
      // emoji and U+2028.
      final escaping = vectors[1]['event'] as Map<String, dynamic>;
      expect(escaping['content'], contains('/'));
      expect(escaping['content'], contains('\u{d55c}\u{ae00}'));
      expect(escaping['content'], contains('\u{1f600}'));
    });

    test('a tagged event reproduces the relay vectors exactly', () async {
      // Person key of the vectors: 0x05 repeated.
      final secret = List.filled(32, '05').join();
      for (final vector
          in (_vectors['event_tags'] as List<dynamic>)
              .cast<Map<String, dynamic>>()) {
        final expected = vector['event'] as Map<String, dynamic>;
        final tags = _tags(expected)
          ..removeWhere((tag) => tag.first == deviceTagName);
        final event = await signDeviceTaggedEvent(
          kind: expected['kind'] as int,
          content: expected['content'] as String,
          tags: tags,
          secretKey: secret,
          createdAt: expected['created_at'] as int,
          device: device,
        );
        expect(event.pubkey, expected['pubkey']);
        expect(event.tags, _tags(expected));
        expect(event.id, expected['id']);
        expect(event.isValid(), isTrue);
      }
    });
  });

  group('wire helpers', () {
    test('DER signatures convert to raw r || s', () {
      // r with a leading sign byte (33 bytes), s short (31 bytes).
      final r = [0x00, 0x80, ...List.filled(31, 0x11)];
      final s = List.filled(31, 0x22);
      final der = Uint8List.fromList([
        0x30,
        2 + r.length + 2 + s.length,
        0x02,
        r.length,
        ...r,
        0x02,
        s.length,
        ...s,
      ]);
      final raw = derToRawP256Signature(der);
      expect(raw, hasLength(64));
      expect(raw.sublist(0, 32), [0x80, ...List.filled(31, 0x11)]);
      expect(raw.sublist(32), [0x00, ...List.filled(31, 0x22)]);
      expect(
        () => derToRawP256Signature(Uint8List.fromList([0x31, 0x00])),
        throwsFormatException,
      );
      expect(
        () => derToRawP256Signature(Uint8List.fromList([...der, 0x00])),
        throwsFormatException,
      );
    });

    test('public keys compress to SEC1 33 bytes', () async {
      final info = await device.info();
      final compressed = info!.publicKey;
      expect(compressed, hasLength(33));
      expect(compressP256PublicKey(compressed), compressed);
      expect(() => compressP256PublicKey(Uint8List(10)), throwsFormatException);
    });

    test('bindings carry the method, full URL and query as sent', () {
      expect(
        requestProofBinding('get', Uri.parse('https://relay.example/auth/me')),
        'GET https://relay.example/auth/me',
      );
      expect(
        requestProofBinding(
          'GET',
          Uri.parse(
            'https://relay.example',
          ).replace(path: '/auth/people/lookup', queryParameters: {'q': 'a b'}),
        ),
        'GET https://relay.example/auth/people/lookup?q=a+b',
      );
      expect(
        requestProofBinding('POST', Uri.parse('http://localhost:3000/auth/x')),
        'POST http://localhost:3000/auth/x',
      );
      expect(
        loginProofBinding('bzl_code'),
        'login:${hexEncode(sha256Bytes(utf8.encode('bzl_code')))}',
      );
    });

    test('relay clocks are kept per origin', () {
      addTearDown(resetServerClockOffset);
      final phone = DateTime.now().millisecondsSinceEpoch ~/ 1000;
      expect(hasServerClock('https://a.example'), isFalse);
      expect(noteServerTime('https://a.example', '${phone + 600}'), isTrue);
      expect(
        noteServerTime('https://b.example:8443/', '${phone - 300}'),
        isTrue,
      );
      expect(
        noteServerTime('https://a.example/auth/me', '${phone + 601}'),
        isFalse,
      );
      expect(
        (proofNow('https://a.example') - (phone + 600)).abs(),
        lessThan(5),
      );
      expect(
        (proofNow('https://b.example:8443') - (phone - 300)).abs(),
        lessThan(5),
      );
      expect((proofNow('https://c.example') - phone).abs(), lessThan(5));
      expect(hasServerClock('https://c.example'), isFalse);
    });

    test('nonces are URL-safe and 16-64 characters', () {
      final nonce = newProofNonce();
      expect(nonce, matches(RegExp(r'^[A-Za-z0-9_-]{16,64}$')));
      expect(newProofNonce(), isNot(nonce));
    });

    test('without a device key, events are signed as before', () async {
      final keys = nostr.Keys.generate();
      final event = await signDeviceTaggedEvent(
        kind: 22242,
        content: '',
        tags: [
          ['challenge', 'c'],
        ],
        secretKey: keys.secret,
        device: _NoDeviceKey(),
      );
      expect(event.tags, [
        ['challenge', 'c'],
      ]);
      expect(event.isValid(), isTrue);
    });

    test('a failed device signature falls back to no tag', () async {
      final failing = SoftwareDeviceKey(List.filled(32, 9))..failSigning = true;
      final event = await signDeviceTaggedEvent(
        kind: 27235,
        content: '',
        tags: const [],
        secretKey: nostr.Keys.generate().secret,
        device: failing,
      );
      expect(event.tags, isEmpty);
    });
  });
}

class _NoDeviceKey implements DeviceKey {
  @override
  Future<DeviceKeyInfo?> info() async => null;

  @override
  Future<Uint8List?> sign(List<int> message) async => null;
}
