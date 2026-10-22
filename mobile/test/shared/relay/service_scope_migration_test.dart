import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/auth/token/refresh_token_store.dart';
import 'package:kuzz/shared/relay/service_address.dart';
import 'package:kuzz/shared/relay/service_scope_migration.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../community/community_storage_test.dart' show FakeSecureStorage;

const _old = 'https://jihobuzz.duckdns.org:8443';
const _oldWs = 'wss://jihobuzz.duckdns.org:8443';
const _pk = 'pk';

ServiceAddresses _moved() => ServiceAddresses.fromDefines(
  environment: 'prod',
  relayUrl: 'wss://jihobuzz.duckdns.org',
  keyBackupOrigin: 'https://jihobuzz.duckdns.org',
  previousRelayUrls: _oldWs,
);

void main() {
  tearDown(() => debugServiceAddressesOverride = null);

  group('serviceScopedKey', () {
    String? scoped(String key) => serviceScopedKey(key, _old, 'kuzz');

    test('replaces every form old key builders wrote', () {
      expect(scoped('kuzz.auth.refresh.v1:$_old'), 'kuzz.auth.refresh.v1:kuzz');
      expect(
        scoped('kuzz.key-backup.pending.v1:${jsonEncode([_old, 'acct'])}'),
        'kuzz.key-backup.pending.v1:${jsonEncode(['kuzz', 'acct'])}',
      );
      expect(
        scoped('kuzz.channel-sort.v1:$_pk:${Uri.encodeComponent(_oldWs)}'),
        'kuzz.channel-sort.v1:$_pk:kuzz',
      );
      expect(scoped('kuzz-drafts:$_oldWs/:$_pk'), 'kuzz-drafts:kuzz:$_pk');
      expect(
        scoped('kuzz.threadPins.v1:HTTPS://JIHOBUZZ.duckdns.org:8443:$_pk'),
        'kuzz.threadPins.v1:kuzz:$_pk',
      );
    });

    test('a hex pubkey starting with a digit after the address', () {
      const pk = '3fa9c0de';
      expect(
        scoped('kuzz.threadPins.v1:$_old:$pk'),
        'kuzz.threadPins.v1:kuzz:$pk',
      );
      expect(scoped('kuzz-drafts:$_oldWs/:$pk'), 'kuzz-drafts:kuzz:$pk');
      expect(
        scoped('k:${Uri.encodeComponent(_old)}%3A12345678ab'),
        'k:kuzz%3A12345678ab',
      );
      expect(
        serviceScopedKey(
          'k:https://jihobuzz.duckdns.org:3fa9c0de',
          'https://jihobuzz.duckdns.org',
          'kuzz',
        ),
        'k:kuzz:3fa9c0de',
      );
    });

    test('does not match other ports, hosts or insecure schemes', () {
      expect(scoped('k:https://jihobuzz.duckdns.org:84430'), isNull);
      expect(scoped('k:https://jihobuzz.duckdns.org'), isNull);
      expect(scoped('k:http://jihobuzz.duckdns.org:8443'), isNull);
      expect(scoped('k:https://jihobuzz.duckdns.org.evil:8443'), isNull);
    });

    test('a default-port origin does not match a longer form', () {
      const origin = 'https://jihobuzz.duckdns.org';
      expect(
        serviceScopedKey(
          'k:https://jihobuzz.duckdns.org:8443:x',
          origin,
          'kuzz',
        ),
        isNull,
      );
      expect(
        serviceScopedKey('k:https://jihobuzz.duckdns.org:x', origin, 'kuzz'),
        'k:kuzz:x',
      );
      expect(
        serviceScopedKey('k:wss://jihobuzz.duckdns.org:443', origin, 'kuzz'),
        'k:kuzz',
      );
    });
  });

  group('migrateServiceScopedStorage', () {
    late FakeSecureStorage secure;

    setUp(() => secure = FakeSecureStorage());

    test('copies old keys, keeps them and never overwrites new keys', () async {
      SharedPreferences.setMockInitialValues({
        'kuzz-drafts:$_old:$_pk': 'old draft',
        'kuzz-drafts:kuzz:$_pk': 'new draft',
        'kuzz-recent-emoji:$_old:$_pk': 'emoji',
        'kuzz.flag:$_old': true,
        'kuzz.count:$_old': 3,
        'kuzz-recent-searches:$_old:$_pk': <String>['a', 'b'],
        'kuzz.other:https://relay.example.com': 'untouched',
      });
      final prefs = await SharedPreferences.getInstance();
      secure['kuzz.auth.refresh.v1:$_old'] = 'token';
      secure['kuzz.auth.key-backup.refresh.v1:kuzz'] = 'newer';
      secure['kuzz.auth.key-backup.refresh.v1:$_old'] = 'older';
      secure['kuzz.key-backup.pending.v1:${jsonEncode([_old, 'acct'])}'] =
          'nsec';

      await migrateServiceScopedStorage(
        prefs: prefs,
        secure: secure,
        addresses: _moved(),
      );

      expect(prefs.getString('kuzz-drafts:kuzz:$_pk'), 'new draft');
      expect(prefs.getString('kuzz-drafts:$_old:$_pk'), 'old draft');
      expect(prefs.getString('kuzz-recent-emoji:kuzz:$_pk'), 'emoji');
      expect(prefs.getString('kuzz-recent-emoji:$_old:$_pk'), 'emoji');
      expect(prefs.getBool('kuzz.flag:kuzz'), isTrue);
      expect(prefs.getInt('kuzz.count:kuzz'), 3);
      expect(prefs.getStringList('kuzz-recent-searches:kuzz:$_pk'), ['a', 'b']);
      expect(prefs.getKeys().where((k) => k.contains('relay.example')), [
        'kuzz.other:https://relay.example.com',
      ]);

      expect(secure['kuzz.auth.refresh.v1:kuzz'], 'token');
      expect(secure['kuzz.auth.refresh.v1:$_old'], 'token');
      expect(secure['kuzz.auth.key-backup.refresh.v1:kuzz'], 'newer');
      expect(secure['kuzz.auth.key-backup.refresh.v1:$_old'], 'older');
      expect(
        secure['kuzz.key-backup.pending.v1:${jsonEncode(['kuzz', 'acct'])}'],
        'nsec',
      );
      expect(
        prefs.getStringList(serviceScopeMigratedKey),
        unorderedEquals([
          'https://jihobuzz.duckdns.org:8443',
          'https://jihobuzz.duckdns.org',
        ]),
      );
    });

    test('the refresh token store reads the migrated record', () async {
      SharedPreferences.setMockInitialValues({});
      final prefs = await SharedPreferences.getInstance();
      secure['kuzz.auth.refresh.v1:$_old'] = jsonEncode({
        'refresh': 'r1',
        'principal_id': 'p1',
      });
      await migrateServiceScopedStorage(
        prefs: prefs,
        secure: secure,
        addresses: _moved(),
      );
      debugServiceAddressesOverride = _moved();
      final store = SecureRefreshTokenStore(storage: secure);
      final session = await store.read('https://jihobuzz.duckdns.org');
      expect(session?.refreshToken, 'r1');
    });

    test('runs once per address: deleted values are not resurrected', () async {
      SharedPreferences.setMockInitialValues({'kuzz-drafts:$_old:$_pk': 'old'});
      final prefs = await SharedPreferences.getInstance();
      secure['kuzz.auth.refresh.v1:$_old'] = 'token';
      Future<void> run() => migrateServiceScopedStorage(
        prefs: prefs,
        secure: secure,
        addresses: _moved(),
      );

      await run();
      await prefs.remove('kuzz-drafts:kuzz:$_pk');
      await secure.delete(key: 'kuzz.auth.refresh.v1:kuzz');
      await run();

      expect(prefs.containsKey('kuzz-drafts:kuzz:$_pk'), isFalse);
      expect(secure['kuzz.auth.refresh.v1:kuzz'], isNull);
      expect(prefs.getString('kuzz-drafts:$_old:$_pk'), 'old');
    });

    test('a newly added previous address is migrated later', () async {
      SharedPreferences.setMockInitialValues({
        serviceScopeMigratedKey: ['https://jihobuzz.duckdns.org:8443'],
        'kuzz-drafts:$_old:$_pk': 'skipped',
        'kuzz-drafts:https://jihobuzz.duckdns.org:$_pk': 'migrated',
      });
      final prefs = await SharedPreferences.getInstance();
      await migrateServiceScopedStorage(
        prefs: prefs,
        secure: secure,
        addresses: ServiceAddresses.fromDefines(
          environment: 'prod',
          relayUrl: 'wss://kuzz.example',
          previousRelayUrls:
              'wss://jihobuzz.duckdns.org:8443,wss://jihobuzz.duckdns.org',
        ),
      );
      expect(prefs.getString('kuzz-drafts:kuzz:$_pk'), 'migrated');
    });

    test('prefers an https-keyed value over its wss twin', () async {
      SharedPreferences.setMockInitialValues({
        'kuzz-drafts:$_oldWs:$_pk': 'stale',
        'kuzz-drafts:$_old:$_pk': 'fresh',
      });
      final prefs = await SharedPreferences.getInstance();
      await migrateServiceScopedStorage(
        prefs: prefs,
        secure: secure,
        addresses: _moved(),
      );
      expect(prefs.getString('kuzz-drafts:kuzz:$_pk'), 'fresh');
    });

    test('does nothing without service addresses', () async {
      SharedPreferences.setMockInitialValues({'kuzz-drafts:$_old:$_pk': 'x'});
      final prefs = await SharedPreferences.getInstance();
      await migrateServiceScopedStorage(
        prefs: prefs,
        secure: secure,
        addresses: ServiceAddresses.fromDefines(environment: 'prod'),
      );
      expect(prefs.getKeys(), {'kuzz-drafts:$_old:$_pk'});
    });
  });
}
