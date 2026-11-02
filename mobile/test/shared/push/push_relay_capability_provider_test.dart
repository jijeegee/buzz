import 'package:kuzz/shared/push/dev_push_lease.dart';
import 'package:kuzz/shared/push/push_relay_capability_provider.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'valid capability starts independent permission and APNs registration',
    () async {
      var requests = 0;

      await startKuzzPushRegistrationIfCapable(
        _descriptor,
        startRegistration: () async {
          requests += 1;
        },
      );

      expect(requests, 1);
    },
  );

  test(
    'missing capability cannot start permission or APNs registration',
    () async {
      var requests = 0;

      await startKuzzPushRegistrationIfCapable(
        null,
        startRegistration: () async {
          requests += 1;
        },
      );

      expect(requests, 0);
    },
  );

  test('a malformed descriptor keeps capability inactive', () async {
    final descriptor = await discoverKuzzPushRelayCapability(
      'https://relay.example',
      fetchDescriptor: (_) async =>
          throw const FormatException('malformed descriptor'),
    );
    var requests = 0;

    await startKuzzPushRegistrationIfCapable(
      descriptor,
      startRegistration: () async {
        requests += 1;
      },
    );

    expect(descriptor, isNull);
    expect(requests, 0);
  });

  test('an unreachable relay is an error to retry, not no capability', () {
    expect(
      discoverKuzzPushRelayCapability(
        'https://relay.example',
        fetchDescriptor: (_) async => throw StateError('relay unreachable'),
      ),
      throwsStateError,
    );
  });
}

const _descriptor = KuzzPushLeaseDescriptor(
  origin: 'wss://relay.example',
  executorKeyId: 'relay-v1',
  executorPubkey:
      'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
  transport: 'apns',
  maxLeaseTtlSeconds: 3600,
  maxContentLength: 4096,
  maxPlaintextLength: 4096,
  maxEndpointLength: 2048,
  maxStringLength: 512,
);
