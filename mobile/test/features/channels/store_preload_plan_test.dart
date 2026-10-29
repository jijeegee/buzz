import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/store_preload_plan.dart';

// Shared rules with desktop (test-fixtures/message-store-preload-plan.json).
void main() {
  final fixture =
      jsonDecode(
            File(
              '../test-fixtures/message-store-preload-plan.json',
            ).readAsStringSync(),
          )
          as Map<String, dynamic>;
  List<String> strings(Object? list) => (list! as List).cast<String>();

  for (final raw in fixture['cases'] as List) {
    final testCase = raw as Map<String, dynamic>;
    test(testCase['name'], () {
      final plan = planStorePreload(
        strings(testCase['rooms']),
        strings(testCase['tokens']).toSet(),
        strings(testCase['certain']).toSet(),
      );
      final expected = testCase['expect'] as Map<String, dynamic>;
      expect(plan.check, strings(expected['check']));
      expect(plan.certain, strings(expected['certain']));
      expect(plan.fetch, strings(expected['fetch']));
      expect(plan.noToken, strings(expected['no_token']));
    });
  }

  test('one log line per pass', () {
    final plan = planStorePreload(['a', 'b', 'c'], {'a', 'b'}, {'a'});
    expect(
      storePreloadSummary(plan),
      '[preload] certain 1 / fetched 2 / skipped (no token) 1',
    );
  });
}
