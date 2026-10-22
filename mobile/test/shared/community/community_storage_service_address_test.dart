import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/community/community_storage.dart';
import 'package:kuzz/shared/relay/service_address.dart';

import 'community_storage_test.dart' show FakeSecureStorage;

void main() {
  late FakeSecureStorage secure;
  late CommunityStorage storage;

  setUp(() {
    secure = FakeSecureStorage();
    storage = CommunityStorage(secure: secure);
  });

  tearDown(() => debugServiceAddressesOverride = null);

  Future<Community> stored(String relayUrl) async {
    final community = Community.create(
      name: 'Kuzz',
      relayUrl: relayUrl,
      pubkey: 'a' * 64,
    );
    await storage.save(community);
    return community;
  }

  test(
    'a row on a previous service address loads on the current one',
    () async {
      final old = await stored('wss://jihobuzz.duckdns.org:8443');
      final other = await stored('https://relay.example.com');
      debugServiceAddressesOverride = ServiceAddresses.fromDefines(
        environment: 'prod',
        relayUrl: 'wss://jihobuzz.duckdns.org',
        previousRelayUrls: 'wss://jihobuzz.duckdns.org:8443',
      );

      final loaded = await storage.loadAll();

      expect(loaded, hasLength(2));
      expect(loaded.first.id, old.id);
      expect(loaded.first.relayUrl, 'wss://jihobuzz.duckdns.org');
      expect(loaded.first.pubkey, old.pubkey);
      expect(loaded.last.relayUrl, other.relayUrl);
    },
  );

  test(
    'a later move rewrites the same row again; saving keeps one row',
    () async {
      final old = await stored('https://jihobuzz.duckdns.org:8443');
      debugServiceAddressesOverride = ServiceAddresses.fromDefines(
        environment: 'prod',
        relayUrl: 'wss://jihobuzz.duckdns.org',
        previousRelayUrls: 'wss://jihobuzz.duckdns.org:8443',
      );
      final first = (await storage.loadAll()).single;
      await storage.save(first.copyWith(name: 'Renamed'));
      expect(
        (await storage.loadAll()).single.relayUrl,
        'https://jihobuzz.duckdns.org',
      );

      debugServiceAddressesOverride = ServiceAddresses.fromDefines(
        environment: 'prod',
        relayUrl: 'wss://kuzz.example:9443',
        previousRelayUrls:
            'wss://jihobuzz.duckdns.org:8443,wss://jihobuzz.duckdns.org',
      );
      final moved = (await storage.loadAll()).single;
      expect(moved.id, old.id);
      expect(moved.name, 'Renamed');
      expect(moved.relayUrl, 'https://kuzz.example:9443');
    },
  );

  test('rows stay unchanged without service addresses', () async {
    await stored('wss://jihobuzz.duckdns.org:8443');
    expect(
      (await storage.loadAll()).single.relayUrl,
      'wss://jihobuzz.duckdns.org:8443',
    );
  });
}
