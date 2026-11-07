import 'package:kuzz/shared/community/community.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('existing records default to device authentication', () {
    final community = Community.fromJson({
      'id': 'one',
      'name': 'Kuzz',
      'relayUrl': 'https://relay.test',
      'addedAt': '2026-08-05T00:00:00.000Z',
    });

    expect(
      community.sensitiveActionPolicy,
      SensitiveActionPolicy.disabledByUser,
    );
    expect(community.starterSetupIncomplete, isFalse);
    expect(community.pushLeaseInstallationId, isNull);
    expect(community.tokenAuth, isFalse);
  });

  test('token auth mode round-trips and is omitted for legacy records', () {
    final token = Community.create(
      name: 'Kuzz',
      relayUrl: 'https://relay.test',
      tokenAuth: true,
    );
    final legacy = Community.create(
      name: 'Kuzz',
      relayUrl: 'https://relay.test',
    );

    expect(token.toJson()['authMode'], 'token');
    expect(Community.fromJson(token.toJson()).tokenAuth, isTrue);
    expect(legacy.toJson().containsKey('authMode'), isFalse);
    expect(token.copyWith(name: 'Renamed').tokenAuth, isTrue);
  });

  test('the cached profile round-trips and is absent on older records', () {
    final older = Community.fromJson({
      'id': 'row',
      'name': 'Relay',
      'relayUrl': 'https://relay.example',
      'addedAt': '2026-01-01T00:00:00.000Z',
    });
    expect(older.displayName, isNull);
    expect(older.avatarUrl, isNull);
    expect(older.email, isNull);
    expect(older.toJson().keys, isNot(contains('email')));

    final cached = Community.fromJson(
      older
          .withCachedProfile(name: ' Jiho ', avatar: 'https://a.example/j.png')
          .copyWith(email: 'jiho@example.com')
          .toJson(),
    );
    expect(cached.displayName, 'Jiho');
    expect(cached.avatarUrl, 'https://a.example/j.png');
    expect(cached.email, 'jiho@example.com');
  });

  test('the cached profile keeps a name, and no oversized photo', () {
    final row = Community.fromJson({
      'id': 'row',
      'name': 'Relay',
      'relayUrl': 'https://relay.example',
      'addedAt': '2026-01-01T00:00:00.000Z',
      'displayName': 'Jiho',
      'avatarUrl': 'https://a.example/j.png',
    });
    final updated = row.withCachedProfile(
      name: '  ',
      avatar: 'data:image/png;base64,${'a' * Community.maxCachedAvatarLength}',
    );
    expect(updated.displayName, 'Jiho');
    expect(updated.avatarUrl, isNull);
  });

  test('push choice round-trips and is absent on legacy records', () {
    final legacy = Community.fromJson({
      'id': 'one',
      'name': 'Kuzz',
      'relayUrl': 'https://relay.test',
      'pushNotificationsEnabled': false,
      'addedAt': '2026-08-05T00:00:00.000Z',
    });
    final off = legacy.copyWith(pushChoice: KuzzPushChoice.off);

    expect(legacy.pushChoice, isNull);
    expect(legacy.toJson().containsKey('pushChoice'), isFalse);
    expect(off.toJson()['pushChoice'], 'off');
    expect(Community.fromJson(off.toJson()).pushChoice, KuzzPushChoice.off);
    expect(off.copyWith(name: 'Renamed').pushChoice, KuzzPushChoice.off);
    expect(off.resumeSession().pushChoice, KuzzPushChoice.off);
  });

  test('new community gets a unique canonical push lease address id', () {
    final first = Community.create(name: 'One', relayUrl: 'https://relay.test');
    final second = Community.create(
      name: 'Two',
      relayUrl: 'https://relay.test',
    );

    expect(first.pushLeaseInstallationId, matches(RegExp(r'^[0-9a-f]{32}$')));
    expect(
      second.pushLeaseInstallationId,
      isNot(first.pushLeaseInstallationId),
    );
  });

  test('community settings round trip', () {
    final community = Community(
      id: 'one',
      name: 'Kuzz',
      relayUrl: 'https://relay.test',
      sensitiveActionPolicy: SensitiveActionPolicy.enabled,
      pushLeaseInstallationId: 'a' * 32,
      starterSetupIncomplete: true,
      addedAt: DateTime.utc(2026, 8, 5),
    );

    final roundTrip = Community.fromJson(community.toJson());
    expect(roundTrip.sensitiveActionPolicy, SensitiveActionPolicy.enabled);
    expect(roundTrip.starterSetupIncomplete, isTrue);
    expect(roundTrip.pushLeaseInstallationId, 'a' * 32);
  });

  test('malformed stored push lease address id is rejected', () {
    expect(
      () => Community.fromJson({
        'id': 'one',
        'name': 'Kuzz',
        'relayUrl': 'https://relay.test',
        'pushLeaseInstallationId': 'not-canonical',
        'addedAt': '2026-08-05T00:00:00.000Z',
      }),
      throwsFormatException,
    );
  });
}
