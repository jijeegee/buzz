import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:buzz/features/channels/agent_activity/observer_models.dart';
import 'package:buzz/features/channels/agent_activity/transcript_builder.dart';
import 'package:buzz/features/channels/agent_activity/transcript_item_widget.dart';
import 'package:buzz/shared/theme/theme.dart';

/// Shared with desktop (`agentSessionObserverSummary.test.mjs`): real buzz-acp
/// captures plus synthetic summary-tier cases, each with the rows both apps
/// must show.
final _fixtures = [
  for (final file
      in Directory('../test-fixtures/observer-summary')
          .listSync()
          .whereType<File>()
          .where((file) => file.path.endsWith('.json'))
          .toList()
        ..sort((a, b) => a.path.compareTo(b.path)))
    (
      name: file.uri.pathSegments.last,
      json: jsonDecode(file.readAsStringSync()) as Map<String, dynamic>,
    ),
];

List<ObserverFrame> _frames(Map<String, dynamic> fixture) => [
  for (final event in fixture['events'] as List)
    ObserverFrame.fromJson(event as Map<String, dynamic>),
];

String _status(ToolStatus status) => switch (status) {
  ToolStatus.executing => 'executing',
  ToolStatus.completed => 'completed',
  ToolStatus.failed => 'failed',
  ToolStatus.pending => 'pending',
};

/// The fixture row projection: assistant messages, tools (summarised tools
/// display their argument preview in place of arguments) and gap lines.
List<Map<String, dynamic>> _projectRows(List<TranscriptItem> items) => [
  for (final item in items)
    if (item is MessageItem && item.role == 'assistant')
      {'type': 'message', 'role': 'assistant', 'text': item.text}
    else if (item is ToolItem)
      {
        'type': 'tool',
        'title': item.title,
        'status': _status(item.status),
        'args': item.argsPreview ?? item.args,
        'result': item.result,
      }
    else if (item is LifecycleItem && item.id.startsWith('gap:'))
      {'type': 'gap', 'text': item.title},
];

/// Huge captured results are pinned by prefix and length.
void _expectRowsMatch(
  List<Map<String, dynamic>> actual,
  List<dynamic> expected,
  String name,
) {
  expect(actual, hasLength(expected.length), reason: '$name: row count');
  for (var index = 0; index < expected.length; index++) {
    final row = expected[index] as Map<String, dynamic>;
    final got = actual[index];
    final result = row['result'];
    if (result is Map) {
      final text = got['result'] as String;
      expect(text, startsWith(result['startsWith'] as String));
      expect(text.length, result['length'], reason: '$name: row $index');
      expect(
        {...got, 'result': null},
        {...row, 'result': null},
        reason: '$name: row $index',
      );
    } else {
      expect(got, row, reason: '$name: row $index');
    }
  }
}

Widget _testable(Widget child) => MaterialApp(
  theme: AppTheme.light(),
  home: Scaffold(body: SingleChildScrollView(child: child)),
);

void main() {
  test('observer summary fixtures are present', () {
    expect(_fixtures.length, greaterThanOrEqualTo(5));
  });

  for (final fixture in _fixtures) {
    final expected = fixture.json['expected'] as Map<String, dynamic>;

    test('observer summary fixture ${fixture.name}: transcript rows', () {
      _expectRowsMatch(
        _projectRows(buildTranscript(_frames(fixture.json))),
        expected['rows'] as List,
        fixture.name,
      );
    });

    test('observer summary fixture ${fixture.name}: summary badge', () {
      expect(
        isObserverSummaryView(_frames(fixture.json)),
        expected['summaryView'],
      );
    });
  }

  test(
    'describeObserverGap covers singular, plural, ellipsis and skipped updates',
    () {
      const cases = <(Map<String, dynamic>, String)>[
        (
          {
            'folded_events': 30,
            'folded_tools': 12,
            'tool_names': ['Read', 'Bash', 'Edit'],
          },
          '12 tools ran in between (Read, Bash, Edit…)',
        ),
        (
          {
            'folded_events': 2,
            'folded_tools': 1,
            'tool_names': ['Read'],
          },
          '1 tool ran in between (Read)',
        ),
        (
          {
            'folded_events': 9,
            'folded_tools': 2,
            'tool_names': ['Read', 'Bash'],
          },
          '2 tools ran in between (Read, Bash)',
        ),
        (
          {'folded_events': 4, 'folded_tools': 3, 'tool_names': <String>[]},
          '3 tools ran in between',
        ),
        (
          {'folded_events': 3, 'folded_tools': 0, 'tool_names': <String>[]},
          '3 updates skipped',
        ),
        (
          {'folded_events': 1, 'folded_tools': 0, 'tool_names': <String>[]},
          '1 update skipped',
        ),
        (<String, dynamic>{}, '0 updates skipped'),
      ];
      for (final (payload, text) in cases) {
        expect(describeObserverGap(payload), text);
      }
    },
  );

  test('isObserverSummaryView follows the latest summarisable frame', () {
    ObserverFrame frame(String kind, [String? detail]) =>
        ObserverFrame(seq: 1, timestamp: '', kind: kind, detail: detail);
    final cases = <(List<ObserverFrame>, bool)>[
      (const [], false),
      ([frame('acp_read')], false),
      ([frame('acp_read', 'free')], true),
      ([frame('acp_read', 'standard')], true),
      ([frame('acp_read', 'premium')], false),
      ([frame('observer_gap', 'free')], true),
      // Lifecycle frames never carry detail and must not reset the view.
      ([frame('acp_read', 'free'), frame('turn_completed')], true),
      // An upgrade to full detail mid-session drops the badge, and vice versa.
      ([frame('acp_read', 'free'), frame('acp_read')], false),
      ([frame('acp_read'), frame('acp_read', 'standard')], true),
    ];
    for (final (frames, expected) in cases) {
      expect(isObserverSummaryView(frames), expected);
    }
  });

  ObserverFrame toolFrame(
    int seq,
    String turnId,
    Map<String, dynamic> update, {
    String? detail = 'free',
  }) => ObserverFrame(
    seq: seq,
    timestamp: '2026-10-10T00:00:0$seq.000Z',
    kind: 'acp_read',
    channelId: 'chan-1',
    sessionId: 'sess-1',
    turnId: turnId,
    detail: detail,
    payload: {
      'method': 'session/update',
      'params': {'sessionId': 'sess-1', 'update': update},
    },
  );
  ObserverFrame turnFrame(int seq, String kind, String turnId) => ObserverFrame(
    seq: seq,
    timestamp: '2026-10-10T00:00:0$seq.000Z',
    kind: kind,
    channelId: 'chan-1',
    turnId: turnId,
    payload: const <String, dynamic>{},
  );

  test('turn_error alone fails the turn\'s running tools', () {
    final items = buildTranscript([
      toolFrame(1, 't1', {
        'sessionUpdate': 'tool_call',
        'toolCallId': 'x',
        'title': 'Build',
        'status': 'in_progress',
      }),
      turnFrame(2, 'turn_error', 't1'),
    ]);
    expect(items.whereType<ToolItem>().single.status, ToolStatus.failed);
  });

  test('a tool\'s own late update clears the turn-end close', () {
    final items = buildTranscript([
      toolFrame(1, 't1', {
        'sessionUpdate': 'tool_call',
        'toolCallId': 'x',
        'title': 'Build',
        'status': 'in_progress',
      }),
      turnFrame(2, 'turn_completed', 't1'),
      toolFrame(3, 't1', {
        'sessionUpdate': 'tool_call_update',
        'toolCallId': 'x',
        'status': 'completed',
      }),
      turnFrame(4, 'turn_error', 't1'),
    ]);
    expect(items.whereType<ToolItem>().single.status, ToolStatus.completed);
  });

  testWidgets('shows a summary preview inline with no result section', (
    tester,
  ) async {
    await tester.pumpWidget(
      _testable(
        TranscriptItemWidget(
          item: ToolItem(
            id: 'tool:x',
            title: 'Run tests',
            toolName: 'run_tests',
            status: ToolStatus.completed,
            args: const {},
            argsPreview: '{"command":"cargo te…',
            result: '',
            isError: false,
            timestamp: '2026-10-10T00:00:01Z',
          ),
        ),
      ),
    );

    expect(find.text('{"command":"cargo te…'), findsOneWidget);
    expect(find.text('Arguments'), findsNothing);
    expect(find.text('Result'), findsNothing);
    expect(find.text('Done'), findsOneWidget);
  });

  testWidgets('renders a gap as one plain line', (tester) async {
    final items = buildTranscript([
      ObserverFrame(
        seq: 4,
        timestamp: '2026-10-10T00:00:04Z',
        kind: 'observer_gap',
        detail: 'free',
        payload: const {
          'folded_events': 30,
          'folded_tools': 12,
          'tool_names': ['Read', 'Bash', 'Edit'],
        },
      ),
    ]);
    await tester.pumpWidget(
      _testable(TranscriptItemWidget(item: items.single)),
    );

    expect(
      find.text('12 tools ran in between (Read, Bash, Edit…)'),
      findsOneWidget,
    );
  });
}
