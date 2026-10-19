import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/relay/media_auth.dart';
import 'package:kuzz/shared/relay/service_address.dart';

ServiceAddresses _moved() => ServiceAddresses.fromDefines(
  environment: 'prod',
  relayUrl: 'wss://jihobuzz.duckdns.org',
  keyBackupOrigin: 'https://jihobuzz.duckdns.org',
  previousRelayUrls: 'wss://jihobuzz.duckdns.org:8443',
);

void main() {
  tearDown(() => debugServiceAddressesOverride = null);

  group('scope', () {
    test('prod (or unset) is kuzz, dev is kuzz-dev', () {
      expect(ServiceAddresses.fromDefines(environment: 'prod').scope, 'kuzz');
      expect(ServiceAddresses.fromDefines(environment: '').scope, 'kuzz');
      expect(
        ServiceAddresses.fromDefines(environment: 'dev').scope,
        'kuzz-dev',
      );
    });
  });

  group('isServiceAddress', () {
    test('matches the current and previous addresses in any form', () {
      final service = _moved();
      for (final url in [
        'wss://jihobuzz.duckdns.org',
        'https://jihobuzz.duckdns.org/',
        'https://jihobuzz.duckdns.org:443',
        'wss://JIHOBUZZ.duckdns.org:443/path?q=1',
        'jihobuzz.duckdns.org',
        'wss://jihobuzz.duckdns.org:8443',
        'https://jihobuzz.duckdns.org:8443/media/a.png',
      ]) {
        expect(service.isServiceAddress(url), isTrue, reason: url);
      }
    });

    test('rejects other hosts, ports, insecure forms and garbage', () {
      final service = _moved();
      for (final url in [
        'wss://jihobuzz.duckdns.org:9443',
        'ws://jihobuzz.duckdns.org',
        'http://jihobuzz.duckdns.org:8443',
        'wss://other.example',
        'wss://jihobuzz.duckdns.org.evil.example',
        'ftp://jihobuzz.duckdns.org',
        '',
        '::::',
      ]) {
        expect(service.isServiceAddress(url), isFalse, reason: url);
      }
    });

    test('nothing is a service address without address defines', () {
      final service = ServiceAddresses.fromDefines(environment: 'prod');
      expect(service.origins, isEmpty);
      expect(service.isServiceAddress('https://relay.example.com'), isFalse);
    });

    test('the current build address counts before any move', () {
      final service = ServiceAddresses.fromDefines(
        environment: 'prod',
        relayUrl: 'wss://jihobuzz.duckdns.org:8443',
        keyBackupOrigin: 'https://jihobuzz.duckdns.org:8443',
        previousRelayUrls: 'wss://jihobuzz.duckdns.org:8443',
      );
      expect(service.origins, ['https://jihobuzz.duckdns.org:8443']);
      expect(service.currentOrigin, 'https://jihobuzz.duckdns.org:8443');
    });

    test('origins are ordered longest first', () {
      expect(_moved().origins, [
        'https://jihobuzz.duckdns.org:8443',
        'https://jihobuzz.duckdns.org',
      ]);
    });
  });

  group('storageScopeFor', () {
    test('service addresses share the scope; others keep their value', () {
      final service = _moved();
      expect(
        service.storageScopeFor('https://jihobuzz.duckdns.org:8443'),
        'kuzz',
      );
      expect(service.storageScopeFor('wss://jihobuzz.duckdns.org'), 'kuzz');
      expect(
        service.storageScopeFor('https://relay.example.com'),
        'https://relay.example.com',
      );
      expect(service.storageScopeFor('not a url'), 'not a url');
    });

    test('app-level helpers follow the override', () {
      expect(
        storageScopeFor('wss://jihobuzz.duckdns.org:8443'),
        'wss://jihobuzz.duckdns.org:8443',
      );
      debugServiceAddressesOverride = _moved();
      expect(storageScopeFor('wss://jihobuzz.duckdns.org:8443'), 'kuzz');
      expect(serviceScope, 'kuzz');
    });
  });

  group('currentServiceUrl', () {
    test(
      'moves previous addresses to the current one, keeping kind and path',
      () {
        final service = _moved();
        expect(
          service.currentServiceUrl('wss://jihobuzz.duckdns.org:8443'),
          'wss://jihobuzz.duckdns.org',
        );
        expect(
          service.currentServiceUrl(
            'https://jihobuzz.duckdns.org:8443/media/abc.png?x=1#f',
          ),
          'https://jihobuzz.duckdns.org/media/abc.png?x=1#f',
        );
        expect(
          service.currentServiceUrl('jihobuzz.duckdns.org:8443/media/a'),
          'jihobuzz.duckdns.org/media/a',
        );
      },
    );

    test('leaves current and foreign URLs unchanged', () {
      final service = _moved();
      for (final url in [
        'wss://jihobuzz.duckdns.org',
        'https://jihobuzz.duckdns.org:443/x',
        'https://cdn.example.com/media/a.png',
        'http://jihobuzz.duckdns.org:8443/media/a.png',
      ]) {
        expect(service.currentServiceUrl(url), url);
      }
    });

    test('moves to a non-default port as well', () {
      final service = ServiceAddresses.fromDefines(
        environment: 'prod',
        relayUrl: 'wss://new.example:9443',
        previousRelayUrls: 'wss://old.example, https://older.example:8443',
      );
      expect(
        service.currentServiceUrl('https://old.example/media/a'),
        'https://new.example:9443/media/a',
      );
      expect(
        service.currentServiceUrl('wss://older.example:8443'),
        'wss://new.example:9443',
      );
    });
  });

  group('media auth', () {
    test('media on a previous service address is relay media', () {
      debugServiceAddressesOverride = _moved();
      final auth = MediaGetAuthService(
        baseUrl: 'https://jihobuzz.duckdns.org',
        nsec: null,
      );
      const old = 'https://jihobuzz.duckdns.org:8443/media/abc.png';
      expect(auth.resolve(old), 'https://jihobuzz.duckdns.org/media/abc.png');
      expect(auth.isRelayMediaUrl(old), isTrue);
      expect(
        auth.isRelayMediaUrl('https://cdn.example.com/media/abc.png'),
        isFalse,
      );
    });
  });
}
