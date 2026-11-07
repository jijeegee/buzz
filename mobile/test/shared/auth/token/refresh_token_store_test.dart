import 'dart:convert';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/auth/token/token.dart';
import 'package:kuzz/shared/relay/service_address.dart';
import 'package:kuzz/shared/relay/service_scope_migration.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../community/community_storage_test.dart' show FakeSecureStorage;
import 'token_auth_test_fakes.dart';

const _origin = 'https://relay.test';
const _principal = 'account-one';
const _other = 'account-two';

String _record(String refresh, String principal) =>
    jsonEncode({'refresh': refresh, 'principal_id': principal});

class _LockedStorage extends FakeSecureStorage {
  Object? readError;
  int readAllCalls = 0;

  @override
  Future<Map<String, String>> readAll({
    AppleOptions? iOptions,
    AndroidOptions? aOptions,
    LinuxOptions? lOptions,
    WebOptions? webOptions,
    AppleOptions? mOptions,
    WindowsOptions? wOptions,
  }) {
    readAllCalls += 1;
    return super.readAll();
  }

  @override
  Future<String?> read({
    required String key,
    AppleOptions? iOptions,
    AndroidOptions? aOptions,
    LinuxOptions? lOptions,
    WebOptions? webOptions,
    AppleOptions? mOptions,
    WindowsOptions? wOptions,
  }) async {
    final error = readError;
    if (error != null) throw error;
    return super.read(key: key);
  }
}

void main() {
  final key = SessionKey(_origin, _principal);
  final v2 = SecureRefreshTokenStore.keyFor(key);
  final v1 = SecureRefreshTokenStore.legacyKeyFor(_origin);

  late _LockedStorage secure;
  late SecureRefreshTokenStore store;

  setUp(() {
    secure = _LockedStorage();
    store = SecureRefreshTokenStore(storage: secure);
  });
  tearDown(() => debugServiceAddressesOverride = null);

  test('the same principal moves to its own record', () async {
    secure[v1] = _record('bzr_old', _principal);

    final session = await store.read(key);

    expect(session?.refreshToken, 'bzr_old');
    expect(secure[v2], isNotNull);
    expect(secure[v1], isNull);
  });

  test('another principal leaves the old record for its own account', () async {
    secure[v1] = _record('bzr_other', _other);

    expect(await store.read(key), isNull);
    expect(secure[v2], isNull);
    expect(secure[v1], isNotNull);

    final other = SessionKey(_origin, _other);
    expect((await store.read(other))?.refreshToken, 'bzr_other');
    expect(secure[v1], isNull);
  });

  test('a record already moved is never overwritten', () async {
    secure[v2] = _record('bzr_new', _principal);
    secure[v1] = _record('bzr_spent', _principal);

    expect((await store.read(key))?.refreshToken, 'bzr_new');
    expect(secure[v1], isNull, reason: 'a spent token must not stay around');
    expect(jsonDecode(secure[v2]!)['refresh'], 'bzr_new');
  });

  test('a moved record keeps an old record of another principal', () async {
    secure[v2] = _record('bzr_new', _principal);
    secure[v1] = _record('bzr_other', _other);

    expect((await store.read(key))?.refreshToken, 'bzr_new');
    expect(secure[v1], isNotNull);
  });

  test('a keychain read error propagates and moves nothing', () async {
    secure[v1] = _record('bzr_old', _principal);
    secure.readError = StateError('keychain locked');

    await expectLater(store.read(key), throwsStateError);

    secure.readError = null;
    expect(secure[v1], isNotNull);
    expect(secure[v2], isNull);
  });

  test('a read error stalls the session, it does not sign out', () async {
    secure[v1] = _record('bzr_old', _principal);
    secure.readError = StateError('keychain locked');
    final server = FakeAuthServer();
    final clock = FakeClock(DateTime.utc(2026, 1, 1));
    final controller = TokenSessionController(
      key: key,
      api: AuthApi(origin: _origin, client: server.client),
      store: store,
      clock: clock.call,
      timerFactory: clock.createTimer,
    );
    addTearDown(controller.dispose);

    await controller.restore();

    expect(controller.state.status, TokenSessionStatus.stalled);
    expect(secure[v1], isNotNull);
  });

  test('an unreadable old record is removed and reads as absent', () async {
    secure[v1] = '{not json';

    expect(await store.read(key), isNull);
    expect(secure[v1], isNull);
    expect(secure[v2], isNull);
  });

  test('an unreadable new record falls back to the old one', () async {
    secure[v2] = '{not json';
    secure[v1] = _record('bzr_old', _principal);

    expect((await store.read(key))?.refreshToken, 'bzr_old');
    expect(secure[v1], isNull);
  });

  test('deleting an account also deletes its record not moved yet', () async {
    secure[v1] = _record('bzr_old', _principal);

    await store.delete(key);

    expect(secure[v1], isNull);
    expect(await store.read(key), isNull);
  });

  test('deleting an account keeps the old record of another', () async {
    secure[v1] = _record('bzr_other', _other);

    await store.delete(key);

    expect(secure[v1], isNotNull);
  });

  test('both stores keep separate records', () async {
    final backup = SecureRefreshTokenStore(
      storage: secure,
      keyPrefix: SecureRefreshTokenStore.keyBackupKeyPrefix,
    );
    secure[SecureRefreshTokenStore.legacyKeyFor(
      _origin,
      prefix: SecureRefreshTokenStore.keyBackupKeyPrefix,
    )] = _record(
      'bzr_backup',
      _principal,
    );

    expect(await store.read(key), isNull);
    expect((await backup.read(key))?.refreshToken, 'bzr_backup');
    expect(
      secure[SecureRefreshTokenStore.keyFor(
        key,
        prefix: SecureRefreshTokenStore.keyBackupKeyPrefix,
      )],
      isNotNull,
    );
  });

  group('with a moved service address', () {
    const old = 'https://jihobuzz.duckdns.org:8443';
    const current = 'https://jihobuzz.duckdns.org';
    final addresses = ServiceAddresses.fromDefines(
      environment: 'prod',
      relayUrl: 'wss://jihobuzz.duckdns.org',
      keyBackupOrigin: current,
      previousRelayUrls: 'wss://jihobuzz.duckdns.org:8443',
    );

    test('every address copy goes, so migration cannot bring a spent token '
        'back', () async {
      SharedPreferences.setMockInitialValues({});
      final prefs = await SharedPreferences.getInstance();
      final addressKey = 'kuzz.auth.refresh.v1:$old';
      secure[addressKey] = _record('bzr_spent', _principal);
      secure['kuzz.auth.refresh.v1:wss://jihobuzz.duckdns.org'] = _record(
        'bzr_spent',
        _principal,
      );
      await migrateServiceScopedStorage(
        prefs: prefs,
        secure: secure,
        addresses: addresses,
      );
      debugServiceAddressesOverride = addresses;
      final moved = SessionKey(current, _principal);
      final scoped = SecureRefreshTokenStore.legacyKeyFor(current);
      expect(secure[scoped], isNotNull);
      final readsBeforeMove = secure.readAllCalls;

      expect((await store.read(moved))?.refreshToken, 'bzr_spent');
      await store.write(
        moved,
        const StoredTokenSession(
          refreshToken: 'bzr_new',
          principalId: _principal,
        ),
      );

      expect(secure[addressKey], isNull);
      expect(secure['kuzz.auth.refresh.v1:wss://jihobuzz.duckdns.org'], isNull);
      expect(secure[scoped], isNull);
      expect(
        secure.readAllCalls,
        readsBeforeMove,
        reason: 'on Android one bad entry in readAll wipes the whole store',
      );

      SharedPreferences.setMockInitialValues({});
      await migrateServiceScopedStorage(
        prefs: await SharedPreferences.getInstance(),
        secure: secure,
        addresses: addresses,
      );
      expect(secure[scoped], isNull);
      expect((await store.read(moved))?.refreshToken, 'bzr_new');
    });
  });
}
