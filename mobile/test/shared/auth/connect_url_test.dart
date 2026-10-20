import 'package:flutter_test/flutter_test.dart';

import 'package:kuzz/shared/auth/account/account_api.dart';
import 'package:kuzz/shared/auth/default_community.dart';

/// The relay WebSocket may follow the address the built-in server gives an
/// account (`/auth/me` `connect_url`); nothing else does.
void main() {
  const ours = 'https://kuzz.example';

  test('a given address must be another WebSocket of the same security', () {
    expect(
      acceptConnectUrl(' wss://jp.kuzz.example ', ours),
      'wss://jp.kuzz.example',
    );
    expect(acceptConnectUrl('wss://kuzz.example/', ours), isNull);
    expect(acceptConnectUrl('ws://jp.kuzz.example', ours), isNull);
    expect(acceptConnectUrl('https://jp.kuzz.example', ours), isNull);
    expect(acceptConnectUrl('jp.kuzz.example', ours), isNull);
    expect(acceptConnectUrl(null, ours), isNull);
    expect(acceptConnectUrl('wss://jp.kuzz.example', null), isNull);
    // A dev build on a local relay follows its unencrypted addresses only.
    expect(
      acceptConnectUrl('ws://localhost:3001', 'http://localhost:3000'),
      'ws://localhost:3001',
    );
    expect(
      acceptConnectUrl('wss://jp.kuzz.example', 'http://localhost:3000'),
      isNull,
    );
  });

  test('only the relay WebSocket guard takes a remembered address', () {
    final previous = debugUnrestrictedServerForTests;
    debugUnrestrictedServerForTests = false;
    addTearDown(() => debugUnrestrictedServerForTests = previous);
    const principal = 'aa';
    const region = 'wss://jp.learned-region.example';
    addTearDown(() => rememberConnectUrl(principal, null));

    expect(allowRelayConnection('test', region), isFalse);
    rememberConnectUrl(principal, region);
    expect(serverGivenConnectUrl(principal), region);
    expect(serverGivenConnectUrl('bb'), isNull);
    expect(allowRelayConnection('test', region), isTrue);
    expect(
      allowRelayConnection('test', 'ws://jp.learned-region.example'),
      isFalse,
    );
    expect(allowConnection('test', region), isFalse);

    rememberConnectUrl(principal, null);
    expect(serverGivenConnectUrl(principal), isNull);
    expect(allowRelayConnection('test', region), isFalse);
  });

  test('/auth/me connect_url is read when sent', () {
    final profile = AccountProfile.fromJson({
      'principal_id': 'aa',
      'display_name': 'A',
      'connect_url': 'wss://jp.kuzz.example',
    });
    expect(profile.connectUrl, 'wss://jp.kuzz.example');
    final older = AccountProfile.fromJson({
      'principal_id': 'aa',
      'display_name': 'A',
    });
    expect(older.connectUrl, isNull);
  });
}
