import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/thread_root_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';

// Shared rules with desktop (test-fixtures/thread-open-root.json).

NostrEvent _event(List<dynamic> spec, String channel) => NostrEvent(
  id: spec[0] as String,
  pubkey: spec[2] as String,
  createdAt: 1,
  kind: spec[1] as int,
  tags: [
    ['h', channel],
    if (spec[3] case final String target) ['e', target],
  ],
  content: '',
  sig: '',
);

void main() {
  final fixture =
      jsonDecode(
            File('../test-fixtures/thread-open-root.json').readAsStringSync(),
          )
          as Map<String, dynamic>;
  final channel = fixture['channel'] as String;
  final root = fixture['root'] as String;
  for (final raw in fixture['cases'] as List) {
    final testCase = raw as Map<String, dynamic>;
    test('shared fixture: ${testCase['name']}', () {
      final answer = (testCase['answer'] as List?)
          ?.map((spec) => _event(spec as List<dynamic>, channel))
          .toList();
      final knownAuthor = testCase['known_root_author'] as String?;
      final result = threadRootVerdict(
        answer,
        channelId: channel,
        rootId: root,
        knownRoot: knownAuthor == null
            ? null
            : _event([root, 9, knownAuthor, null], channel),
      );
      expect(result.verdict.name, testCase['verdict']);
      if (result.verdict == ThreadRootVerdict.found) {
        expect(result.root?.id, root);
      }
    });
  }
}
