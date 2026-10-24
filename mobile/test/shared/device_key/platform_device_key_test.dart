import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/device_key/device_key.dart';
import 'package:pointycastle/export.dart';

import '../../helpers/software_device_key.dart';

const _channel = MethodChannel('kuzz/device_key');

/// DER `SEQUENCE { INTEGER r, INTEGER s }` of a raw `r || s` signature, as
/// the Android Keystore and the iOS Secure Enclave return it.
Uint8List _der(Uint8List raw) {
  List<int> integer(List<int> value) {
    var bytes = value.skipWhile((byte) => byte == 0).toList();
    if (bytes.isEmpty || bytes.first & 0x80 != 0) bytes = [0, ...bytes];
    return [0x02, bytes.length, ...bytes];
  }

  final body = [...integer(raw.sublist(0, 32)), ...integer(raw.sublist(32))];
  return Uint8List.fromList([0x30, body.length, ...body]);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  final software = SoftwareDeviceKey(List<int>.filled(32, 7));
  final uncompressed = Uint8List.fromList(
    ECCurve_secp256r1().curve
        .decodePoint(software.publicKey)!
        .getEncoded(false),
  );

  tearDown(() => messenger.setMockMethodCallHandler(_channel, null));

  /// The channel contract both native plugins implement.
  void answerLikeTheNativePlugins({required bool hardware}) {
    messenger.setMockMethodCallHandler(_channel, (call) async {
      switch (call.method) {
        case 'getOrCreate':
          return {'publicKey': uncompressed, 'hardware': hardware};
        case 'sign':
          final message = (call.arguments as Map)['message'] as Uint8List;
          return _der((await software.sign(message))!);
      }
      return null;
    });
  }

  test('reads the public key and kind from the platform', () async {
    answerLikeTheNativePlugins(hardware: true);
    final key = PlatformDeviceKey(supported: true);
    final info = await key.info();
    expect(info!.publicKey, software.publicKey);
    expect(info.kind, 'hardware');
  });

  test('reports a key outside secure hardware as software', () async {
    answerLikeTheNativePlugins(hardware: false);
    final info = await PlatformDeviceKey(supported: true).info();
    expect(info!.kind, 'software');
  });

  test('turns the DER signature into raw r || s', () async {
    answerLikeTheNativePlugins(hardware: true);
    final message = 'kuzz-device-proof/v1\nauth'.codeUnits;
    final signature = await PlatformDeviceKey(supported: true).sign(message);
    expect(signature, hasLength(64));
    expect(
      verifyDeviceSignature(software.publicKey, message, signature!),
      isTrue,
    );
  });

  test('a platform without the plugin has no device key', () async {
    final key = PlatformDeviceKey(supported: true);
    expect(await key.info(), isNull);
    expect(await key.sign([1, 2, 3]), isNull);
  });

  test('a native failure is retried on the next call', () async {
    var calls = 0;
    messenger.setMockMethodCallHandler(_channel, (call) async {
      calls++;
      if (calls == 1) {
        throw PlatformException(code: 'device_key_failed');
      }
      return {'publicKey': uncompressed, 'hardware': true};
    });
    final key = PlatformDeviceKey(supported: true);
    expect(await key.info(), isNull);
    expect((await key.info())!.publicKey, software.publicKey);
    expect(calls, 2);
  });
}
