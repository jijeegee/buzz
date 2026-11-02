import 'dart:convert';
import 'dart:typed_data';

import 'package:nostr/nostr.dart' as nostr;
import 'package:pointycastle/digests/sha256.dart';
import 'package:uuid/uuid.dart';

import '../device_key/device_proof.dart';

/// A NIP-98 `Authorization` header for one relay HTTP request, signed by
/// the person key and, when this phone has one, by its device key (the
/// `device` tag, device-bound keys plan §2.1). Every header carries a fresh
/// `nonce`, so two headers for the same request in the same second differ
/// (the relay refuses a replayed id).
///
/// Plain Dart with no provider state: the background push handler, which
/// runs in its own isolate, signs its `/query` with it too.
Future<String> buildNip98AuthHeader({
  required String method,
  required String url,
  required List<int> bodyBytes,
  required String? nsec,
}) async {
  if (nsec == null || nsec.isEmpty) {
    throw Exception('Cannot query relay: no signing key available');
  }
  final privkeyHex = nostr.Nip19.decode(payload: nsec).data;
  if (privkeyHex.isEmpty) {
    throw Exception('Invalid nsec');
  }
  final payloadHash = SHA256Digest()
      .process(Uint8List.fromList(bodyBytes))
      .map((byte) => byte.toRadixString(16).padLeft(2, '0'))
      .join();
  final event = await signDeviceTaggedEvent(
    kind: 27235,
    content: '',
    tags: [
      ['u', url],
      ['method', method.toUpperCase()],
      ['payload', payloadHash],
      ['nonce', const Uuid().v4()],
    ],
    secretKey: privkeyHex,
  );
  return 'Nostr ${base64.encode(utf8.encode(event.toJson()))}';
}
