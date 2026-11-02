import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/push/push_lease_matcher.dart';
import 'package:kuzz/shared/push/push_subscription.dart';

/// The `matcher` cases of test-fixtures/push-inbox.json, as the Swift
/// `PushInboxResolverTests` reads them.
void main() {
  final fixture =
      jsonDecode(File('../test-fixtures/push-inbox.json').readAsStringSync())
          as Map<String, dynamic>;
  final policies = [
    for (final raw in fixture['matcher_policies'] as List)
      KuzzPushSubscription.fromJson({
        ...Map<String, dynamic>.from(raw as Map),
        'class': 'default',
      }),
  ];

  for (final raw in fixture['matcher'] as List) {
    final c = raw as Map<String, dynamic>;
    test(c['name'] as String, () {
      final event = {
        'id': '0' * 64,
        'created_at': 1,
        'content': '',
        'sig': '',
        ...Map<String, dynamic>.from(c['event'] as Map),
      };
      expect(pushPoliciesMatch(policies, event), c['match']);
      if (c.containsKey('keep_as_profile')) {
        expect(pushKeepsAsProfile(event), c['keep_as_profile']);
      }
    });
  }

  test('tag values compare case-insensitively, as on iPhone', () {
    final event = {
      'kind': 9,
      'pubkey': 'b' * 64,
      'tags': [
        ['h', '11111111-1111-4111-8111-111111111111'.toUpperCase()],
      ],
    };
    expect(pushPoliciesMatch(policies, event), isTrue);
  });
}
