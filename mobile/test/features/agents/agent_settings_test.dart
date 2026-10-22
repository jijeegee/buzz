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

  group('patchedAgentContent', () {
    test('applies the draft and keeps every other key in order', () {
      final loaded = {
        'name': 'Fizz',
        'source_preset_id': 'fizz-def',
        'system_prompt': 'old',
        'model': 'opus',
        'provider': 'anthropic',
        'parallelism': 2,
        'respond_to': 'owner-only',
        'description': 'old description',
        'future_field': {'x': 1},
      };
      final encoded = patchedAgentContent(
        loaded,
        _draft(
          name: '  Buzzy  ',
          description: '  Helps out  ',
          instructions: '  Be kind.\n',
          model: ' sonnet ',
        ),
      );
      final content = jsonDecode(encoded) as Map<String, dynamic>;
      expect(content, {
        'name': 'Buzzy',
        'source_preset_id': 'fizz-def',
        'system_prompt': '  Be kind.\n',
        'model': 'sonnet',
        'provider': 'anthropic',
        'parallelism': 2,
        'respond_to': 'owner-only',
        'description': 'Helps out',
        'future_field': {'x': 1},
      });
      expect(content.keys.toList(), loaded.keys.toList());
      expect(loaded['name'], 'Fizz', reason: 'input is not mutated');
    });

    test('leaves blank description and model out', () {
      final content =
          jsonDecode(
                patchedAgentContent({
                  'name': 'Fizz',
                  'system_prompt': 'x',
                  'model': 'opus',
                  'description': 'x',
                }, _draft(description: '   ', model: ' ')),
              )
              as Map<String, dynamic>;
      expect(content.containsKey('model'), isFalse);
      expect(content.containsKey('description'), isFalse);
      // An emptied prompt is written as empty, never dropped.
      expect(content['system_prompt'], '');
    });
  });

  group('managedAgentSourcePresetId', () {
    test('reads source_preset_id, then an older persona_id', () {
      expect(
        managedAgentSourcePresetId(
          jsonEncode({'source_preset_id': 'new', 'persona_id': 'old'}),
        ),
        'new',
      );
      expect(
        managedAgentSourcePresetId(jsonEncode({'persona_id': 'old'})),
        'old',
      );
      expect(
        managedAgentSourcePresetId(
          jsonEncode({'source_preset_id': ' ', 'persona_id': 'old'}),
        ),
        'old',
      );
      expect(managedAgentSourcePresetId(jsonEncode({'name': 'x'})), isNull);
      expect(managedAgentSourcePresetId('not json'), isNull);
    });
  });

  test('agentContentEditable needs the agent\'s own instructions', () {
    expect(agentContentEditable({'name': 'x', 'system_prompt': ''}), isTrue);
    expect(
      agentContentEditable({'name': 'x', 'persona_id': 'fizz-def'}),
      isFalse,
    );
    expect(agentContentEditable({'name': 'x', 'system_prompt': null}), isFalse);
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
      String? sourcePresetD = 'fizz-def',
    }) => agentNameConflicts(
      name: name,
      owner: _owner,
      agentPubkey: _fizz,
      sourcePresetD: sourcePresetD,
      agentEvents: agents,
      personaEvents: personas,
    );

    test('finds another agent or preset by name', () {
      final honey = _event(
        kind: EventKind.managedAgent,
        d: _honey,
        content: {'name': 'Honey', 'source_preset_id': 'honey-def'},
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

    test('ignores this agent and the preset it was made from', () {
      final self = _event(
        kind: EventKind.managedAgent,
        d: _fizz,
        content: {'name': 'Fizz'},
      );
      final preset = _event(
        kind: EventKind.persona,
        d: 'fizz-def',
        content: {'display_name': 'Fizz'},
      );
      expect(conflicts('fizz', agents: [self], personas: [preset]), isFalse);
      expect(
        conflicts('fizz', personas: [preset], sourcePresetD: null),
        isTrue,
      );
    });

    test('another agent from the same preset still holds its name', () {
      // Agents are no longer renamed along with their preset.
      final sibling = _event(
        kind: EventKind.managedAgent,
        d: _honey,
        content: {'name': 'Fizz', 'persona_id': 'Fizz-Def'},
      );
      expect(conflicts('Fizz', agents: [sibling]), isTrue);
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
