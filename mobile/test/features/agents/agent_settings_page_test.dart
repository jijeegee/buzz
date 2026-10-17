import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/agents/agent_settings_page.dart';
import 'package:kuzz/features/agents/agent_settings_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme.dart';

final _owner = 'a' * 64;
final _fizz = '1' * 64;
final _honey = '2' * 64;

NostrEvent _event(
  int kind,
  String d,
  Map<String, Object?> content, {
  int createdAt = 100,
  List<List<String>> extraTags = const [],
}) => NostrEvent(
  id: '$kind-$d-$createdAt',
  pubkey: _owner,
  createdAt: createdAt,
  kind: kind,
  tags: [
    ['d', d],
    ...extraTags,
  ],
  content: jsonEncode(content),
);

class _FakeRelay implements AgentSettingsRelay {
  _FakeRelay(this.events);

  /// Mutable so a test can replace the definition mid-edit.
  List<NostrEvent> events;
  final published =
      <({int kind, String content, List<List<String>> tags, int createdAt})>[];

  @override
  Future<List<NostrEvent>> fetch(NostrFilter filter) async {
    final ds = filter.tags['#d'];
    return [
      for (final event in events)
        if (filter.kinds.contains(event.kind) &&
            (ds == null || ds.contains(event.getTagValue('d'))))
          event,
    ];
  }

  @override
  Future<void> publish({
    required int kind,
    required String content,
    required List<List<String>> tags,
    required int createdAt,
  }) async {
    published.add((
      kind: kind,
      content: content,
      tags: tags,
      createdAt: createdAt,
    ));
  }
}

final _definition = _event(
  EventKind.persona,
  'fizz-def',
  {
    'display_name': 'Fizz',
    'system_prompt': 'Be helpful.',
    'model': 'opus',
    'name_pool': ['x'],
    'unknown': true,
  },
  createdAt: 4102444800, // far future: created_at must step past it
  extraTags: const [
    ['shared', 'true'],
  ],
);

final _fizzAgent = _event(EventKind.managedAgent, _fizz, {
  'name': 'Fizz',
  'persona_id': 'Fizz-Def',
});

void main() {
  Future<_FakeRelay> pump(WidgetTester tester, List<NostrEvent> events) async {
    final relay = _FakeRelay(events);
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          myPubkeyProvider.overrideWithValue(_owner),
          agentSettingsRelayProvider.overrideWithValue(relay),
        ],
        child: MaterialApp(
          theme: AppTheme.light(),
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                onPressed: () => Navigator.of(context).push(
                  MaterialPageRoute<void>(
                    builder: (_) => AgentSettingsPage(
                      agentPubkey: _fizz,
                      fallbackName: 'Fizz from profile',
                    ),
                  ),
                ),
                child: const Text('open'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();
    return relay;
  }

  testWidgets('an agent without a definition is read-only', (tester) async {
    await pump(tester, [
      _event(EventKind.managedAgent, _fizz, {
        'name': 'Fizz',
        'persona_id': null,
      }),
    ]);
    expect(find.byKey(const Key('agent-settings-read-only')), findsOneWidget);
    expect(find.text('Fizz'), findsOneWidget);
    expect(find.text(agentSettingsReadOnlyMessage), findsOneWidget);
    expect(find.byType(TextField), findsNothing);
  });

  testWidgets('a missing definition is read-only too', (tester) async {
    await pump(tester, [_fizzAgent]);
    expect(find.text(agentSettingsReadOnlyMessage), findsOneWidget);
  });

  testWidgets('save publishes the patched definition', (tester) async {
    final relay = await pump(tester, [_fizzAgent, _definition]);
    final save = find.byKey(const Key('agent-settings-save'));
    expect(tester.widget<TextButton>(save).onPressed, isNull);

    await tester.enterText(
      find.byKey(const Key('agent-settings-name')),
      ' Buzzy ',
    );
    await tester.enterText(
      find.byKey(const Key('agent-settings-description')),
      'Helps out',
    );
    await tester.enterText(find.byKey(const Key('agent-settings-model')), '');
    await tester.pump();
    await tester.tap(save);
    await tester.pumpAndSettle();

    expect(relay.published, hasLength(1));
    final event = relay.published.single;
    expect(event.kind, EventKind.persona);
    expect(event.tags, [
      ['d', 'fizz-def'],
      ['shared', 'true'],
    ]);
    expect(event.createdAt, _definition.createdAt + 1);
    expect(jsonDecode(event.content), {
      'display_name': 'Buzzy',
      'system_prompt': 'Be helpful.',
      'name_pool': ['x'],
      'description': 'Helps out',
      'unknown': true,
    });
    expect(
      find.text('Saved. Buzzy picks up the change on its computer.'),
      findsOneWidget,
    );
    expect(find.byType(AgentSettingsPage), findsNothing);
  });

  testWidgets('a differently named agent keeps its name', (tester) async {
    // "Fizz 2" of definition "Fizz": renaming the definition would rename
    // the other agent, so the name is shown but not editable here.
    final relay = await pump(tester, [
      _event(EventKind.managedAgent, _fizz, {
        'name': 'Fizz 2',
        'persona_id': 'Fizz-Def',
      }),
      _definition,
    ]);
    expect(find.byKey(const Key('agent-settings-name')), findsNothing);
    expect(find.byKey(const Key('agent-settings-name-locked')), findsOneWidget);
    expect(find.text('Fizz 2'), findsOneWidget);
    await tester.enterText(
      find.byKey(const Key('agent-settings-description')),
      'Helps out',
    );
    await tester.pump();
    await tester.tap(find.byKey(const Key('agent-settings-save')));
    await tester.pumpAndSettle();
    expect(jsonDecode(relay.published.single.content)['display_name'], 'Fizz');
    expect(
      find.text('Saved. Fizz 2 picks up the change on its computer.'),
      findsOneWidget,
    );
  });

  testWidgets('a taken name is refused', (tester) async {
    final relay = await pump(tester, [
      _fizzAgent,
      _definition,
      _event(EventKind.managedAgent, _honey, {'name': 'Honey'}),
    ]);
    await tester.enterText(
      find.byKey(const Key('agent-settings-name')),
      'honey',
    );
    await tester.pump();
    await tester.tap(find.byKey(const Key('agent-settings-save')));
    await tester.pumpAndSettle();
    expect(relay.published, isEmpty);
    expect(
      find.text(
        'You already have an agent named "honey". Choose a different name.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('a definition changed elsewhere is not overwritten', (
    tester,
  ) async {
    final relay = await pump(tester, [_fizzAgent, _definition]);
    relay.events = [
      _fizzAgent,
      _event(EventKind.persona, 'fizz-def', {
        'display_name': 'Fizz 2',
      }, createdAt: 4102444900),
    ];
    await tester.enterText(
      find.byKey(const Key('agent-settings-instructions')),
      'New rules.',
    );
    await tester.pump();
    await tester.tap(find.byKey(const Key('agent-settings-save')));
    await tester.pumpAndSettle();
    expect(relay.published, isEmpty);
    expect(find.text(agentSettingsChangedMessage), findsOneWidget);
  });
}
