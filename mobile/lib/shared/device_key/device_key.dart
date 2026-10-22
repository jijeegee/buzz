import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'device_proof.dart';

/// This install's device key: a P-256 key pair kept in the Android Keystore
/// (StrongBox when the phone has one, else the TEE). The private key never
/// leaves the secure hardware; the app only asks it for signatures, so a
/// person key or token copied to another phone is useless there
/// (device-bound keys plan §2.1, PR D).
///
/// iOS and other platforms have no implementation yet: [info] and [sign]
/// return `null`, and callers then send no device proof (as older apps do).
abstract class DeviceKey {
  /// The public key and storage kind, creating the key on first use, or
  /// `null` when this platform has no device key.
  Future<DeviceKeyInfo?> info();

  /// A raw `r || s` (64-byte) ECDSA P-256/SHA-256 signature over [message],
  /// or `null` when this platform has no device key or signing failed.
  Future<Uint8List?> sign(List<int> message);

  /// The process-wide device key. Tests replace it and restore it after.
  static DeviceKey instance = PlatformDeviceKey();
}

/// The public half of the device key.
@immutable
class DeviceKeyInfo {
  const DeviceKeyInfo({required this.publicKey, required this.hardware});

  /// SEC1 compressed public key (33 bytes).
  final Uint8List publicKey;

  /// Whether the key lives in secure hardware (StrongBox or TEE).
  final bool hardware;

  /// Lower-case hex of [publicKey], as the relay expects it.
  String get publicKeyHex => hexEncode(publicKey);

  /// The relay's `device_key_kind`: `hardware` or `software`.
  String get kind => hardware ? 'hardware' : 'software';
}

/// [DeviceKey] backed by the `kuzz/device_key` platform channel (Android).
class PlatformDeviceKey implements DeviceKey {
  PlatformDeviceKey({MethodChannel? channel, bool? supported})
    : _channel = channel ?? const MethodChannel('kuzz/device_key'),
      _supported = supported ?? (!kIsWeb && Platform.isAndroid);

  final MethodChannel _channel;
  bool _supported;
  Future<DeviceKeyInfo?>? _info;

  @override
  Future<DeviceKeyInfo?> info() {
    if (!_supported) return Future.value();
    return _info ??= _loadInfo().then((value) {
      // A failed load is retried on the next call; a key is cached for good.
      if (value == null) _info = null;
      return value;
    });
  }

  Future<DeviceKeyInfo?> _loadInfo() async {
    try {
      final reply = await _channel.invokeMapMethod<String, Object?>(
        'getOrCreate',
      );
      final raw = reply?['publicKey'];
      if (raw is! Uint8List) return null;
      return DeviceKeyInfo(
        publicKey: compressP256PublicKey(raw),
        hardware: reply?['hardware'] == true,
      );
    } on MissingPluginException {
      _supported = false;
      return null;
    } catch (error) {
      // Never log key material; the error names only the failure.
      debugPrint('[DeviceKey] unavailable: ${error.runtimeType}');
      return null;
    }
  }

  @override
  Future<Uint8List?> sign(List<int> message) async {
    if (!_supported) return null;
    try {
      final der = await _channel.invokeMethod<Uint8List>('sign', {
        'message': Uint8List.fromList(message),
      });
      if (der == null) return null;
      return derToRawP256Signature(der);
    } on MissingPluginException {
      _supported = false;
      return null;
    } catch (error) {
      debugPrint('[DeviceKey] signing failed: ${error.runtimeType}');
      return null;
    }
  }
}
