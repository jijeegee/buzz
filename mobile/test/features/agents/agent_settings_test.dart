import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/agents/agent_settings.dart';
import 'package:kuzz/shared/relay/relay.dart';

final _owner = 'a' * 64;
final _fizz = '1' * 64;
final _honey = '2' * 64;

NostrEvent _event({
  required int kind,
  required String d,
  required Map<String, Object?> content,
  int createdAt = 100,
  String? id,
  String? pubkey,
}) => NostrEvent(
  id: id ?? '$kind-$d-$createdAt',
  pubkey: pubkey ?? _owner,
  createdAt: createdAt,
  kind: kind,
  tags: [
    ['d', d],
  ],
  content: jsonEncode(content),
);

AgentSettingsDraft _draft({
  String name = 'Fizz',
  String description = '',
  String instructions = '',
  String model = '',
}) => AgentSettingsDraft(
  name: name,
  description: description,
  instructions: instructions,
  model: model,
);

void main() {
  group('personaDTag', () {
    test('mirrors desktop normalize_d_tag', () {
      expect(personaDTag('3f2a-uuid'), '3f2a-uuid');
      expect(personaDTag('CodeReviewer'), 'codereviewer');
      expect(personaDTag('custom:reviewer'), 'custom-reviewer');
      expect(personaDTag('_ops'), 'a_ops');
      expect(personaDTag('-x'), 'a-x');
      expect(personaDTag('ünï'), 'a-n-');
      expect(personaDTag('x' * 80), 'x' * 64);
      expect(personaDTag('_${'x' * 80}'), 'a_${'x' * 62}');
    });
  });

  group('patchedPersonaContent', () {
    test('applies the draft and keeps every other key', () {
      final loaded = {
        'display_name': 'Fizz',
        'system_prompt': 'old',
        'acp_command': 'claude-agent-acp',
        'model': 'opus',
        'name_pool': ['a', 'b'],
        'parallelism': 2,
        'description': 'old description',
        'future_field': {'x': 1},
      };
      final content =
          jsonDecode(
                patchedPersonaContent(
                  loaded,
                  _draft(
                    name: '  Buzzy  ',
                    description: '  Helps out  ',
                    instructions: '  Be kind.\n',
                    model: ' sonnet ',
                  ),
                ),
              )
              as Map<String, dynamic>;
      expect(content, {
        'display_name': 'Buzzy',
        'system_prompt': '  Be kind.\n',
        'acp_command': 'claude-agent-acp',
        'model': 'sonnet',
        'name_pool': ['a', 'b'],
        'parallelism': 2,
        'description': 'Helps out',
        'future_field': {'x': 1},
      });
      // Desktop field order first, unknown keys after.
      expect(content.keys.first, 'display_name');
      expect(content.keys.last, 'future_field');
    });

    test('leaves blank description and model out', () {
      final content =
          jsonDecode(
                patchedPersonaContent({
                  'display_name': 'Fizz',
                  'model': 'opus',
                  'description': 'x',
                }, _draft(description: '   ', model: ' ')),
              )
              as Map<String, dynamic>;
      expect(content.containsKey('model'), isFalse);
      expect(content.containsKey('description'), isFalse);
      // No prompt before and none now: stays absent.
      expect(content.containsKey('system_prompt'), isFalse);
    });

    test('keeps an existing empty prompt', () {
      final content =
          jsonDecode(
                patchedPersonaContent({
                  'display_name': 'Fizz',
                  'system_prompt': 'x',
                }, _draft()),
              )
              as Map<String, dynamic>;
      expect(content['system_prompt'], '');
    });
  });

  test('copyTags copies d, shared and other tags', () {
    final tags = [
      ['d', 'slug'],
      ['shared', 'true'],
      ['x', 'y', 'z'],
    ];
    final copy = copyTags(tags);
    expect(copy, tags);
    expect(identical(copy.first, tags.first), isFalse);
  });

  test('nextCreatedAt stays strictly newer', () {
    expect(nextCreatedAt(previous: 100, now: 200), 200);
    expect(nextCreatedAt(previous: 200, now: 200), 201);
    expect(nextCreatedAt(previous: 300, now: 200), 301);
  });

  group('validateAgentSettings', () {
    test('accepts plain text and emoji sequences', () {
      for (final emoji in [
        '❤️',
        '☕️',
        '👩‍💻',
        '🧑🏽‍💻',
        '👨‍👩‍👧‍👦',
        '1\u{FE0F}⃣',
      ]) {
        expect(
          validateAgentSettings(
            _draft(
              name: 'Reviewer $emoji',
              description: 'Helps $emoji',
              instructions: 'Review.\n\tCall out risks $emoji',
            ),
          ),
          isNull,
          reason: emoji,
        );
      }
    });

    test('requires a name within 128 characters', () {
      expect(validateAgentSettings(_draft(name: '  ')), 'Name is required.');
      expect(validateAgentSettings(_draft(name: 'a' * 128)), isNull);
      expect(validateAgentSettings(_draft(name: 'a' * 129)), isNotNull);
      // Characters, not UTF-16 units.
      expect(validateAgentSettings(_draft(name: '🐝' * 128)), isNull);
    });

    test('limits description and instructions', () {
      expect(validateAgentSettings(_draft(description: 'a' * 280)), isNull);
      expect(validateAgentSettings(_draft(description: 'a' * 281)), isNotNull);
      expect(
        validateAgentSettings(_draft(instructions: 'a' * (64 * 1024))),
        isNull,
      );
      // 64 KiB in UTF-8 bytes: 3 bytes each.
      expect(
        validateAgentSettings(_draft(instructions: '가' * 21846)),
        isNotNull,
      );
    });

    test('rejects invisible and control characters', () {
      for (final char in [
        '\u00AD',
        '\u200B',
        '\u202E',
        '\u2066',
        '\u3164',
        '\u{E007F}',
        '\u0000',
        '\r',
        '\u0007',
        '\u0085',
      ]) {
        expect(
          validateAgentSettings(_draft(name: 'Re${char}viewer')),
          isNotNull,
          reason: 'name U+${char.runes.first.toRadixString(16)}',
        );
        expect(
          validateAgentSettings(_draft(description: 'A${char}b')),
          isNotNull,
        );
        expect(
          validateAgentSettings(_draft(instructions: 'Review$char code')),
          isNotNull,
        );
      }
      expect(
        validateAgentSettings(_draft(name: 'Review\u{FE0F}er')),
        isNotNull,
      );
      expect(validateAgentSettings(_draft(name: 'Review\u200Der')), isNotNull);
      expect(validateAgentSettings(_draft(name: '\uFEFFFizz')), isNotNull);
      expect(validateAgentSettings(_draft(description: 'a\nb')), isNotNull);
    });
  });

  group('agentNameConflicts', () {
    bool conflicts(
      String name, {
      List<NostrEvent> agents = const [],
      List<NostrEvent> personas = const [],
    }) => agentNameConflicts(
      name: name,
      owner: _owner,
      agentPubkey: _fizz,
      personaD: 'fizz-def',
      agentEvents: agents,
      personaEvents: personas,
    );

    test('finds another agent or definition by name', () {
      final honey = _event(
        kind: EventKind.managedAgent,
        d: _honey,
        content: {'name': 'Honey', 'persona_id': 'honey-def'},
      );
      expect(conflicts(' honey ', agents: [honey]), isTrue);
      expect(conflicts('Pollen', agents: [honey]), isFalse);
      final pollen = _event(
        kind: EventKind.persona,
        d: 'pollen-def',
        content: {'display_name': 'Pollen'},
      );
      expect(conflicts('POLLEN', personas: [pollen]), isTrue);
    });

    test('ignores this agent, its definition and agents sharing it', () {
      expect(
        conflicts(
          'Buzzy',
          agents: [
            _event(
              kind: EventKind.managedAgent,
              d: _fizz,
              content: {'name': 'Buzzy'},
            ),
            _event(
              kind: EventKind.managedAgent,
              d: _honey,
              content: {'name': 'Buzzy', 'persona_id': 'Fizz-Def'},
            ),
          ],
          personas: [
            _event(
              kind: EventKind.persona,
              d: 'fizz-def',
              content: {'display_name': 'Buzzy'},
            ),
          ],
        ),
        isFalse,
      );
    });

    test('uses only the newest event per d tag and the owner\'s', () {
      final old = _event(
        kind: EventKind.managedAgent,
        d: _honey,
        content: {'name': 'Buzzy'},
        createdAt: 100,
      );
      final renamed = _event(
        kind: EventKind.managedAgent,
        d: _honey,
        content: {'name': 'Honey'},
        createdAt: 200,
      );
      expect(conflicts('Buzzy', agents: [old, renamed]), isFalse);
      final stranger = _event(
        kind: EventKind.persona,
        d: 'x',
        content: {'display_name': 'Buzzy'},
        pubkey: 'b' * 64,
      );
      expect(conflicts('Buzzy', personas: [stranger]), isFalse);
    });
  });
}
