import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/agent_activity/observer_subscription.dart';

/// Shared with desktop's observerLiveFilter test (docs/message-sync.md).
void main() {
  test('shared fixture: observer lookback and limit match desktop', () {
    final fixture =
        jsonDecode(
              File('../test-fixtures/agent-observer.json').readAsStringSync(),
            )
            as Map<String, dynamic>;
    expect(observerLiveLookbackSeconds, fixture['lookbackSecs']);
    expect(observerLiveLimit, fixture['limit']);
  });
}
