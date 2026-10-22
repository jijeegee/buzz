import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:crypto/crypto.dart' as crypto;
import 'package:nostr/nostr.dart' as nostr;

import 'device_key.dart';

/// Wire formats of device proofs, byte-identical to the relay's
/// `kuzz-auth/src/device_proof.rs` (checked against its shared
/// `device_proof_vectors.json`).

/// Domain separator of every header proof message.
const _proofDomain = 'kuzz-device-proof/v1';

/// Domain separator of event tag proofs.
const _eventDomain = 'kuzz-device-event/v1';

/// Name of the tag carrying an auth event's device proof.
const deviceTagName = 'device';

/// Request header carrying a header proof (`<ts>.<nonce>.<sig>`).
const deviceProofHeader = 'X-Kuzz-Device-Proof';

/// Response header with the relay clock (unix seconds).
const serverTimeHeader = 'x-kuzz-server-time';

/// Purpose of a proof on an authenticated `/auth/*` call.
const proofPurposeAuth = 'auth';

/// Purpose of a proof on `/auth/refresh`.
const proofPurposeRefresh = 'refresh';

/// Purpose of a login proof (binds the device key to the new session).
const proofPurposeBind = 'bind';

/// Lower-case hex.
String hexEncode(List<int> bytes) {
  final out = StringBuffer();
  for (final byte in bytes) {
    out.write(byte.toRadixString(16).padLeft(2, '0'));
  }
  return out.toString();
}

/// SHA-256 of [bytes].
Uint8List sha256Bytes(List<int> bytes) =>
    Uint8List.fromList(crypto.sha256.convert(bytes).bytes);

/// The exact text a device signs for a header proof:
///
/// ```text
/// kuzz-device-proof/v1
/// <purpose>
/// <binding>
/// <token sha256 hex, or "-">
/// <body sha256 hex, or "-">
/// <ts>
/// <nonce>
/// ```
String headerProofMessage({
  required String purpose,
  required String binding,
  List<int>? tokenHash,
  List<int>? bodyHash,
  required int ts,
  required String nonce,
}) {
  String hash(List<int>? value) => value == null ? '-' : hexEncode(value);
  return [
    _proofDomain,
    purpose,
    binding,
    hash(tokenHash),
    hash(bodyHash),
    '$ts',
    nonce,
  ].join('\n');
}

/// What an account call's proof is bound to: `<METHOD> <URL>` with the path
/// and query exactly as sent (the relay rebuilds it from its public URL and
/// the request target).
String requestProofBinding(String method, Uri url) {
  final path = url.path.isEmpty ? '/' : url.path;
  final query = url.hasQuery ? '?${url.query}' : '';
  return '${method.toUpperCase()} ${url.scheme}://${url.authority}$path$query';
}

/// The binding of a login proof: `login:<sha256(login code) hex>`.
String loginProofBinding(String loginCode) =>
    'login:${hexEncode(sha256Bytes(utf8.encode(loginCode)))}';

/// The exact text a device signs for an auth event: the domain, a newline,
/// then the NIP-01 id serialization `[0, pubkey, created_at, kind, tags,
/// content]` with the signature field of the `device` tag blanked.
///
/// Dart's `jsonEncode` matches NIP-01 (and serde_json) escaping: only `"`,
/// `\`, `\b`, `\f`, `\n`, `\r`, `\t` and other characters below U+0020 (as
/// lower-case `\u00xx`) are escaped; `/`, U+007F, U+2028 and non-ASCII stay
/// raw.
String eventProofMessage({
  required String pubkey,
  required int createdAt,
  required int kind,
  required List<List<String>> tags,
  required String content,
}) {
  final blanked = [
    for (final tag in tags)
      if (tag.isNotEmpty && tag.first == deviceTagName && tag.length > 2)
        [tag[0], tag[1], '', ...tag.skip(3)]
      else
        tag,
  ];
  return '$_eventDomain\n${jsonEncode([0, pubkey, createdAt, kind, blanked, content])}';
}

/// Convert a DER `SEQUENCE { INTEGER r, INTEGER s }` ECDSA signature (what
/// the Android Keystore returns) to raw 64-byte `r || s`.
Uint8List derToRawP256Signature(Uint8List der) {
  var index = 0;
  int byte() {
    if (index >= der.length) {
      throw const FormatException('Truncated DER signature');
    }
    return der[index++];
  }

  int length() {
    final first = byte();
    if (first < 0x80) return first;
    final count = first & 0x7f;
    if (count < 1 || count > 2) {
      throw const FormatException('Bad DER length');
    }
    var value = 0;
    for (var i = 0; i < count; i++) {
      value = (value << 8) | byte();
    }
    return value;
  }

  Uint8List integer() {
    if (byte() != 0x02) throw const FormatException('Expected DER integer');
    final size = length();
    if (size < 1 || index + size > der.length) {
      throw const FormatException('Bad DER integer');
    }
    var start = index;
    final end = index + size;
    index = end;
    while (start < end - 1 && der[start] == 0) {
      start++;
    }
    final value = der.sublist(start, end);
    if (value.length > 32) throw const FormatException('DER integer too big');
    return Uint8List(32)..setRange(32 - value.length, 32, value);
  }

  if (byte() != 0x30) throw const FormatException('Expected DER sequence');
  final total = length();
  if (index + total != der.length) {
    throw const FormatException('Bad DER sequence length');
  }
  final r = integer();
  final s = integer();
  if (index != der.length) throw const FormatException('Trailing DER bytes');
  return Uint8List(64)
    ..setRange(0, 32, r)
    ..setRange(32, 64, s);
}

/// SEC1 compressed form (33 bytes) of a P-256 public key given compressed
/// or uncompressed (`04 || x || y`).
Uint8List compressP256PublicKey(Uint8List key) {
  if (key.length == 33 && (key[0] == 0x02 || key[0] == 0x03)) return key;
  if (key.length == 65 && key[0] == 0x04) {
    final odd = key[64].isOdd;
    return Uint8List(33)
      ..[0] = odd ? 0x03 : 0x02
      ..setRange(1, 33, key, 1);
  }
  throw const FormatException('Not a SEC1 P-256 public key');
}

final _random = Random.secure();

/// A fresh proof nonce: 24 URL-safe characters (18 random bytes).
String newProofNonce() {
  final bytes = List<int>.generate(18, (_) => _random.nextInt(256));
  return base64Url.encode(bytes);
}

/// Seconds each relay's clock is ahead of this phone, learned from
/// [serverTimeHeader] and keyed by origin; proofs are stamped in the time
/// of the relay they go to.
final Map<String, int> _serverClockOffsets = {};

String _clockKey(String origin) {
  final uri = Uri.tryParse(origin);
  if (uri == null || !uri.hasAuthority) return origin;
  try {
    return uri.origin;
  } on StateError {
    return origin;
  }
}

/// Record [origin]'s clock from a response header value. Returns whether
/// the known offset (0 before any) moved by more than [threshold] seconds.
bool noteServerTime(String origin, String? header, {int threshold = 30}) {
  final server = int.tryParse(header?.trim() ?? '');
  if (server == null) return false;
  final key = _clockKey(origin);
  final offset = server - _phoneSeconds();
  final moved = (offset - (_serverClockOffsets[key] ?? 0)).abs() > threshold;
  _serverClockOffsets[key] = offset;
  return moved;
}

/// Whether [origin]'s clock has been learned.
bool hasServerClock(String origin) =>
    _serverClockOffsets.containsKey(_clockKey(origin));

int _phoneSeconds() => DateTime.now().millisecondsSinceEpoch ~/ 1000;

/// Now in [origin]'s relay time (unix seconds); phone time while unknown.
int proofNow(String origin) =>
    _phoneSeconds() + (_serverClockOffsets[_clockKey(origin)] ?? 0);

/// Forget every learned relay clock (tests).
void resetServerClockOffset() => _serverClockOffsets.clear();

/// Sign a header proof with [device] for the relay at [origin] (its clock
/// stamps the proof): `<ts>.<nonce>.<sig hex>`, or `null` when this phone
/// has no device key or signing failed.
Future<String?> signHeaderProof({
  required DeviceKey device,
  required String purpose,
  required String binding,
  List<int>? tokenHash,
  List<int>? bodyHash,
  required String origin,
  int? ts,
  String? nonce,
}) async {
  if (await device.info() == null) return null;
  final stamp = ts ?? proofNow(origin);
  final value = nonce ?? newProofNonce();
  final message = headerProofMessage(
    purpose: purpose,
    binding: binding,
    tokenHash: tokenHash,
    bodyHash: bodyHash,
    ts: stamp,
    nonce: value,
  );
  final signature = await device.sign(utf8.encode(message));
  if (signature == null) return null;
  return '$stamp.$value.${hexEncode(signature)}';
}

/// Sign an auth event (NIP-42 22242, NIP-98 27235, Blossom 24242, huddle
/// AUTH) with the person key [secretKey] (hex), adding the
/// `["device", <device pubkey>, <sig>]` tag when this phone has a device
/// key. Without one (or if the device signature fails) the event is signed
/// exactly as before, with no device tag.
Future<nostr.Event> signDeviceTaggedEvent({
  required int kind,
  required String content,
  required List<List<String>> tags,
  required String secretKey,
  int? createdAt,
  bool verify = false,
  DeviceKey? device,
}) async {
  final stamp = createdAt ?? DateTime.now().millisecondsSinceEpoch ~/ 1000;
  final key = device ?? DeviceKey.instance;
  final info = await key.info();
  if (info != null) {
    final pubkey = nostr.Keys(secretKey).public.toLowerCase();
    final tagged = [
      ...tags,
      [deviceTagName, info.publicKeyHex, ''],
    ];
    final message = eventProofMessage(
      pubkey: pubkey,
      createdAt: stamp,
      kind: kind,
      tags: tagged,
      content: content,
    );
    final signature = await key.sign(utf8.encode(message));
    if (signature != null) {
      tagged.last[2] = hexEncode(signature);
      return nostr.Event.from(
        kind: kind,
        content: content,
        tags: tagged,
        secretKey: secretKey,
        createdAt: stamp,
        pubkey: pubkey,
        verify: verify,
      );
    }
  }
  return nostr.Event.from(
    kind: kind,
    content: content,
    tags: tags,
    secretKey: secretKey,
    createdAt: stamp,
    verify: verify,
  );
}
