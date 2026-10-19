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

/// The preset Fizz was made from; never edited from agent settings.
final _preset = _event(EventKind.persona, 'fizz-def', {
  'display_name': 'Fizz',
  'system_prompt': 'Preset prompt.',
});

final _fizzAgent = _event(
  EventKind.managedAgent,
  _fizz,
  {
    'name': 'Fizz',
    'system_prompt': 'Be helpful.',
    'model': 'opus',
    'provider': 'anthropic',
    'parallelism': 2,
    'respond_to': 'owner-only',
    'source_preset_id': 'Fizz-Def',
    'unknown': true,
  },
  createdAt: 4102444800, // far future: created_at must step past it
  extraTags: const [
    ['shared', 'true'],
  ],
);

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

  testWidgets('an agent not on the relay is read-only', (tester) async {
    await pump(tester, [_preset]);
    expect(find.byKey(const Key('agent-settings-read-only')), findsOneWidget);
    expect(find.text('Fizz from profile'), findsOneWidget);
    expect(find.text(agentSettingsReadOnlyMessage), findsOneWidget);
    expect(find.byType(TextField), findsNothing);
  });

  testWidgets('an agent linked by an older computer is read-only', (
    tester,
  ) async {
    // Its event names the preset but carries no instructions of its own;
    // saving it would publish a blank prompt.
    final relay = await pump(tester, [
      _event(EventKind.managedAgent, _fizz, {
        'name': 'Fizz',
        'persona_id': 'Fizz-Def',
        'parallelism': 1,
      }),
      _preset,
    ]);
    expect(find.byKey(const Key('agent-settings-read-only')), findsOneWidget);
    expect(find.text('Fizz'), findsOneWidget);
    expect(find.text(agentSettingsUpdateComputerMessage), findsOneWidget);
    expect(find.byType(TextField), findsNothing);
    expect(relay.published, isEmpty);
  });

  testWidgets('save publishes the agent\'s own patched event', (tester) async {
    final relay = await pump(tester, [_fizzAgent, _preset]);
    final save = find.byKey(const Key('agent-settings-save'));
    expect(tester.widget<TextButton>(save).onPressed, isNull);
    expect(find.text('Be helpful.'), findsOneWidget);

    await tester.enterText(
      find.byKey(const Key('agent-settings-name')),
      ' Buzzy ',
    );
    await tester.enterText(
      find.byKey(const Key('agent-settings-description')),
      'Helps out',
    );
    await tester.enterText(
      find.byKey(const Key('agent-settings-instructions')),
      'New rules.',
    );
    await tester.enterText(find.byKey(const Key('agent-settings-model')), '');
    await tester.pump();
    await tester.tap(save);
    await tester.pumpAndSettle();

    expect(relay.published, hasLength(1));
    final event = relay.published.single;
    expect(event.kind, EventKind.managedAgent);
    expect(event.tags, [
      ['d', _fizz],
      ['shared', 'true'],
    ]);
    expect(event.createdAt, _fizzAgent.createdAt + 1);
    expect(jsonDecode(event.content), {
      'name': 'Buzzy',
      'system_prompt': 'New rules.',
      'provider': 'anthropic',
      'parallelism': 2,
      'respond_to': 'owner-only',
      'source_preset_id': 'Fizz-Def',
      'unknown': true,
      'description': 'Helps out',
    });
    expect(relay.published.where((e) => e.kind == EventKind.persona), isEmpty);
    expect(
      find.text('Saved. Buzzy picks up the change on its computer.'),
      findsOneWidget,
    );
    expect(find.byType(AgentSettingsPage), findsNothing);
  });

  testWidgets('an older persona_id label still edits the agent', (
    tester,
  ) async {
    final relay = await pump(tester, [
      _event(EventKind.managedAgent, _fizz, {
        'name': 'Fizz 2',
        'persona_id': 'Fizz-Def',
        'system_prompt': 'Own prompt.',
      }),
      _preset,
    ]);
    await tester.enterText(
      find.byKey(const Key('agent-settings-name')),
      'Fizz',
    );
    await tester.pump();
    await tester.tap(find.byKey(const Key('agent-settings-save')));
    await tester.pumpAndSettle();
    // Sharing the name of the preset it was made from is fine.
    expect(relay.published.single.kind, EventKind.managedAgent);
    expect(jsonDecode(relay.published.single.content), {
      'name': 'Fizz',
      'persona_id': 'Fizz-Def',
      'system_prompt': 'Own prompt.',
    });
  });

  testWidgets('a taken name is refused', (tester) async {
    final relay = await pump(tester, [
      _fizzAgent,
      _preset,
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

  testWidgets('an agent changed elsewhere is not overwritten', (tester) async {
    final relay = await pump(tester, [_fizzAgent, _preset]);
    relay.events = [
      _event(EventKind.managedAgent, _fizz, {
        'name': 'Fizz 2',
        'system_prompt': 'Other.',
      }, createdAt: 4102444900),
      _preset,
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
