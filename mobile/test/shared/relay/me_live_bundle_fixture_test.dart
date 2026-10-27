import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/relay/me_live_bundle.dart';
import 'package:kuzz/shared/relay/relay.dart';

/// Shared with desktop's relayMeBundle tests: both clients must decide which
/// filters ride the account bundle and what each listener receives the same
/// way (docs/message-sync.md).
void main() {
  final fixtures =
      jsonDecode(
            File('../test-fixtures/me-live-bundle.json').readAsStringSync(),
          )
          as Map<String, dynamic>;

  test('shared fixture: kinds and lookback match desktop', () {
    expect(meAuthoredKinds, fixtures['authoredKinds']);
    expect(meAddressedKinds, fixtures['addressedKinds']);
    expect(meCatchUpLookbackSeconds, fixtures['catchUpLookbackSecs']);
  });

  for (final fixture in fixtures['scopes'] as List) {
    test('shared fixture: ${fixture['name']}', () {
      final scope = meScopeOf(_filter(fixture['filter'] as Map));
      final expected = fixture['scope'] as Map?;
      if (expected == null) {
        expect(scope, isNull);
      } else {
        expect(scope?.kind.name, expected['kind']);
        expect(scope?.pubkey, expected['pubkey']);
      }
    });
  }

  for (final fixture in fixtures['matching'] as List) {
    test('shared fixture: ${fixture['name']}', () {
      expect(
        eventMatchesFilter(
          _event(fixture['event'] as Map),
          _filter(fixture['filter'] as Map),
        ),
        fixture['matches'],
      );
    });
  }
}

NostrFilter _filter(Map json) => NostrFilter(
  kinds: [for (final kind in json['kinds'] as List) kind as int],
  authors: (json['authors'] as List?)?.cast<String>(),
  limit: json['limit'] as int,
  since: json['since'] as int?,
  tags: {
    for (final MapEntry(:key, :value) in json.entries)
      if ((key as String).startsWith('#')) key: (value as List).cast<String>(),
  },
);

NostrEvent _event(Map json) => NostrEvent(
  id: json['id'] as String,
  pubkey: json['pubkey'] as String,
  createdAt: json['created_at'] as int,
  kind: json['kind'] as int,
  tags: [for (final tag in json['tags'] as List) (tag as List).cast<String>()],
  content: '',
  sig: '',
);
