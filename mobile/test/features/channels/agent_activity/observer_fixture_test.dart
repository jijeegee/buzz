import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/agent_activity/observer_subscription.dart';
import 'package:kuzz/shared/relay/me_live_bundle.dart';

/// Shared with desktop's observerLiveFilter test (docs/message-sync.md).
void main() {
  final fixture =
      jsonDecode(
            File('../test-fixtures/agent-observer.json').readAsStringSync(),
          )
          as Map<String, dynamic>;
  final owner = fixture['ownerPubkey'] as String;

  test('shared fixture: the observer filter matches desktop', () {
    expect(observerLiveFilter(owner).toJson(), fixture['filter']);
  });

  test('the observer filter rides the addressed account bundle', () {
    final scope = meScopeOf(observerLiveFilter(owner));
    expect(scope?.kind, MeScopeKind.addressed);
    expect(scope?.pubkey, owner);
  });
}
