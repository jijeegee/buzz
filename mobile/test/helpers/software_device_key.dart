import 'dart:async';
import 'dart:typed_data';

import 'package:kuzz/shared/device_key/device_key.dart';
import 'package:kuzz/shared/device_key/device_proof.dart';
import 'package:pointycastle/export.dart';

final _curve = ECCurve_secp256r1();

BigInt _toBigInt(List<int> bytes) =>
    BigInt.parse(hexEncode(bytes).isEmpty ? '0' : hexEncode(bytes), radix: 16);

Uint8List _fixed32(BigInt value) {
  final hex = value.toRadixString(16).padLeft(64, '0');
  return Uint8List.fromList([
    for (var i = 0; i < 64; i += 2)
      int.parse(hex.substring(i, i + 2), radix: 16),
  ]);
}

/// A P-256 [DeviceKey] held in memory (RFC 6979 signatures), standing in
/// for the Android Keystore in tests.
class SoftwareDeviceKey implements DeviceKey {
  SoftwareDeviceKey(List<int> secret, {this.hardware = true})
    : _secret = _toBigInt(secret);

  final BigInt _secret;
  final bool hardware;

  /// Signatures made so far.
  int signCount = 0;

  /// Make [sign] fail (as a Keystore error would).
  bool failSigning = false;

  Uint8List get publicKey =>
      Uint8List.fromList((_curve.G * _secret)!.getEncoded(true));

  @override
  Future<DeviceKeyInfo?> info() async =>
      DeviceKeyInfo(publicKey: publicKey, hardware: hardware);

  /// While set, [sign] waits for it (slow secure hardware).
  Completer<void>? hold;

  @override
  Future<Uint8List?> sign(List<int> message) async {
    final wait = hold;
    if (wait != null) await wait.future;
    if (failSigning) return null;
    signCount++;
    final signer = ECDSASigner(SHA256Digest(), HMac(SHA256Digest(), 64))
      ..init(true, PrivateKeyParameter(ECPrivateKey(_secret, _curve)));
    final signature =
        signer.generateSignature(Uint8List.fromList(message)) as ECSignature;
    return Uint8List.fromList([
      ..._fixed32(signature.r),
      ..._fixed32(signature.s),
    ]);
  }
}

/// Whether [signature] (raw `r || s`) by SEC1 [publicKey] verifies over
/// [message] (ECDSA P-256/SHA-256).
bool verifyDeviceSignature(
  List<int> publicKey,
  List<int> message,
  List<int> signature,
) {
  if (signature.length != 64) return false;
  final point = _curve.curve.decodePoint(publicKey);
  final verifier = ECDSASigner(SHA256Digest())
    ..init(false, PublicKeyParameter(ECPublicKey(point, _curve)));
  return verifier.verifySignature(
    Uint8List.fromList(message),
    ECSignature(
      _toBigInt(signature.sublist(0, 32)),
      _toBigInt(signature.sublist(32)),
    ),
  );
}

/// Decode lower-case hex.
Uint8List hexDecode(String hex) => Uint8List.fromList([
  for (var i = 0; i < hex.length; i += 2)
    int.parse(hex.substring(i, i + 2), radix: 16),
]);
