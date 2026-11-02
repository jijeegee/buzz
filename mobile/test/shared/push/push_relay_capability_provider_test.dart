import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/community/community_provider.dart';
import 'package:kuzz/shared/push/dev_push_lease.dart';
import 'package:kuzz/shared/push/push_relay_capability_provider.dart';
import 'package:kuzz/shared/relay/relay_provider.dart' show Env;
import 'package:kuzz/shared/relay/relay_session.dart';
import 'package:nostr/nostr.dart' as nostr;

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

  test('an HTTP error from the server is an exception, so it is retried', () {
    expect(
      fetchKuzzPushLeaseDescriptor(
        'https://relay.example',
        client: MockClient((_) async => http.Response('busy', 503)),
      ),
      throwsA(
        isA<KuzzRelayInformationHttpException>()
            .having((error) => error, 'error', isA<Exception>())
            .having((error) => error.statusCode, 'statusCode', 503),
      ),
    );
  });

  group('currentRelayPushDescriptorProvider', () {
    late ProviderContainer container;
    var fetches = 0;

    ProviderContainer createContainer(
      Future<KuzzPushLeaseDescriptor> Function(String relayBaseUrl) fetch, {
      SessionStatus status = SessionStatus.connected,
    }) {
      _Session.initial = status;
      _ActiveCommunity.initial = Community.create(
        name: 'Team',
        relayUrl: 'wss://relay.example',
        nsec: nostr.Keys.generate().nsec,
      ).copyWith(pushNotificationsEnabled: true);
      fetches = 0;
      container = ProviderContainer(
        overrides: [
          relaySessionProvider.overrideWith(_Session.new),
          activeCommunityProvider.overrideWith(
            (ref) async => ref.watch(_activeCommunity),
          ),
          kuzzPushDescriptorFetcherProvider.overrideWithValue((url) {
            fetches += 1;
            return fetch(url);
          }),
        ],
      );
      addTearDown(container.dispose);
      container.listen(currentRelayPushDescriptorProvider, (_, _) {});
      return container;
    }

    Future<AsyncValue<KuzzPushLeaseDescriptor?>> settle() async {
      for (var i = 0; i < 100; i++) {
        final value = container.read(currentRelayPushDescriptorProvider);
        if (value.hasValue && !value.isLoading) return value;
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      return container.read(currentRelayPushDescriptorProvider);
    }

    test(
      'waits for the connection, then asks the server',
      () async {
        createContainer(
          (_) async => _descriptor,
          status: SessionStatus.connecting,
        );
        await Future<void>.delayed(const Duration(milliseconds: 50));
        expect(
          container.read(currentRelayPushDescriptorProvider).isLoading,
          true,
        );
        expect(fetches, 0);

        (container.read(relaySessionProvider.notifier) as _Session).setStatus(
          SessionStatus.connected,
        );
        final value = await settle();
        expect(value.value, _descriptor);
        expect(fetches, 1);
      },
      skip: !Env.pushGatewayConfigured,
    );

    test(
      'an HTTP error is retried instead of staying unfinished',
      () async {
        var responses = 0;
        createContainer(
          (url) => fetchKuzzPushLeaseDescriptor(
            url,
            client: MockClient((_) async {
              responses += 1;
              return responses == 1
                  ? http.Response('busy', 503)
                  : http.Response(jsonEncode(_relayInformation), 200);
            }),
          ),
        );
        final value = await settle();
        expect(value.hasError, isFalse);
        expect(value.value?.executorKeyId, 'relay-v1');
        expect(responses, 2);
      },
      skip: !Env.pushGatewayConfigured,
    );

    test(
      'push bookkeeping writes do not ask the server again',
      () async {
        createContainer((_) async => _descriptor);
        await settle();
        expect(fetches, 1);

        final notifier = container.read(_activeCommunity.notifier);
        for (var generation = 1; generation <= 3; generation++) {
          notifier.reserve(generation);
          await Future<void>.delayed(const Duration(milliseconds: 20));
        }
        await settle();
        expect(fetches, 1);
      },
      skip: !Env.pushGatewayConfigured,
    );
  });
}

class _Session extends RelaySessionNotifier {
  static SessionStatus initial = SessionStatus.connected;

  @override
  SessionState build() => SessionState(status: initial);

  void setStatus(SessionStatus status) => state = SessionState(status: status);
}

final _activeCommunity = NotifierProvider<_ActiveCommunity, Community?>(
  _ActiveCommunity.new,
);

class _ActiveCommunity extends Notifier<Community?> {
  static Community? initial;

  @override
  Community? build() => initial;

  void reserve(int generation) => state = state!.copyWith(
    pushSubscriptionState: state!.pushSubscriptionState.withReservedGeneration(
      generation,
    ),
  );
}

final _relayInformation = <String, dynamic>{
  'supported_extensions': ['nip-pl'],
  'push': {
    'origin': 'wss://relay.example',
    'keys': [
      {'id': 'relay-v1', 'pubkey': 'a' * 64, 'current': true},
    ],
    'app_profiles': [
      {'id': kuzzPushAppProfile, 'transport': kuzzPushTransport},
    ],
    'push_kinds': [9, 40002, 45001, 45003],
    'h_grammar': 'uuid-v4-lowercase',
    'class_support': {
      kuzzPushTransport: ['default'],
    },
    'limitation': {
      'max_lease_ttl': 2592000,
      'max_leases_per_pubkey': 16,
      'max_subscriptions_per_lease': 16,
      'max_kinds': 16,
      'max_authors': 20,
      'max_h': 50,
      'max_tag_values': 20,
      'max_ignore': 8,
      'max_content_len': 65536,
      'max_plaintext_len': 32768,
      'max_endpoint_len': 4096,
      'max_string_len': 512,
    },
  },
};

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
