import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/agents/my_teams.dart';
import 'package:kuzz/shared/relay/relay.dart';

final _owner = 'a' * 64;
final _other = 'b' * 64;
final _fizz = '1' * 64;
final _honey = '2' * 64;

var _seq = 0;

NostrEvent _event(
  int kind,
  String d,
  Map<String, dynamic> content, {
  String? author,
  int createdAt = 100,
}) => NostrEvent(
  id: (++_seq).toRadixString(16).padLeft(64, '0'),
  pubkey: author ?? _owner,
  createdAt: createdAt,
  kind: kind,
  tags: [
    ['d', d],
  ],
  content: jsonEncode(content),
  sig: '0' * 128,
);

void main() {
  test('reads teams with member names and agents', () {
    final teams = myTeamPresetsFrom(
      owner: _owner,
      teamEvents: [
        _event(EventKind.team, 'team-b', {
          'name': ' Writers ',
          'description': '  ',
          'persona_ids': ['Scribe'],
        }),
        _event(EventKind.team, 'team-a', {
          'name': 'Research',
          'description': 'Finds sources',
          'persona_ids': ['fizz', 'gone', 'fizz', 'builtin:honey'],
        }),
        // Another owner's team is not ours.
        _event(EventKind.team, 'team-c', {'name': 'Theirs'}, author: _other),
        // A team without a name is skipped.
        _event(EventKind.team, 'team-d', {'persona_ids': <String>[]}),
      ],
      personaEvents: [
        _event(EventKind.persona, 'fizz', {'display_name': 'Fizz'}),
        _event(EventKind.persona, 'scribe', {'display_name': 'Scribe'}),
      ],
      agentEvents: [
        _event(EventKind.managedAgent, _honey, {'persona_id': 'fizz'}),
        _event(EventKind.managedAgent, _fizz, {'persona_id': 'fizz'}),
      ],
    );
    expect([for (final t in teams) t.name], ['Research', 'Writers']);
    final research = teams.first;
    expect(research.id, 'team-a');
    expect(research.description, 'Finds sources');
    expect(
      [for (final m in research.members) m.personaD],
      ['fizz', 'gone', 'builtin-honey'],
    );
    expect(research.members.first.name, 'Fizz');
    expect(research.members.first.agentPubkeys, [_fizz, _honey]);
    expect(research.members[1].name, isNull);
    expect(research.members[1].agentPubkeys, isEmpty);
    expect(research.members.last.name, 'Honey');
    final writers = teams.last;
    expect(writers.description, isNull);
    expect(writers.members.single.personaD, 'scribe');
    expect(writers.members.single.name, 'Scribe');
  });

  test('uses the newest version of a team', () {
    final teams = myTeamPresetsFrom(
      owner: _owner,
      teamEvents: [
        _event(EventKind.team, 't', {'name': 'Old'}, createdAt: 1),
        _event(EventKind.team, 't', {'name': 'New'}, createdAt: 2),
      ],
      personaEvents: const [],
      agentEvents: const [],
    );
    expect([for (final t in teams) t.name], ['New']);
  });

  test('reads agent presets with how many agents use each', () {
    final presets = myAgentPresetsFrom(
      owner: _owner,
      personaEvents: [
        _event(EventKind.persona, 'scribe', {
          'display_name': 'Scribe',
          'description': ' Writes docs ',
        }),
        _event(EventKind.persona, 'fizz', {'display_name': 'Fizz'}),
        // No name: skipped. Another owner's: not ours.
        _event(EventKind.persona, 'blank', {'display_name': ' '}),
        _event(EventKind.persona, 'x', {'display_name': 'X'}, author: _other),
      ],
      agentEvents: [
        _event(EventKind.managedAgent, _fizz, {'persona_id': 'fizz'}),
        _event(EventKind.managedAgent, _honey, {'persona_id': 'fizz'}),
      ],
    );
    expect([for (final p in presets) p.name], ['Fizz', 'Scribe']);
    expect(presets.first.agentCount, 2);
    expect(presets.first.description, isNull);
    expect(presets.last.agentCount, 0);
    expect(presets.last.description, 'Writes docs');
    expect(presetUsageLabel(0), 'not used yet');
    expect(presetUsageLabel(1), 'used by 1 agent');
    expect(presetUsageLabel(2), 'used by 2 agents');
  });

  test('reads teams of real agents, newest of each, sorted by name', () {
    final teams = myAgentTeamsFrom(
      owner: _owner,
      teamEvents: [
        _event(EventKind.agentTeam, 't1', {
          'v': 1,
          'name': 'Old',
          'agent_pubkeys': [_fizz],
        }, createdAt: 1),
        _event(EventKind.agentTeam, 't1', {
          'v': 1,
          'name': ' Build ',
          'description': ' Ships it ',
          'agent_pubkeys': [_honey, _fizz.toUpperCase(), _honey, 'nope'],
        }),
        _event(EventKind.agentTeam, 't2', {
          'v': 1,
          'name': 'Docs',
          'agent_pubkeys': <String>[],
        }),
        // No name: skipped. Another owner's: not ours.
        _event(EventKind.agentTeam, 't3', {'v': 1, 'name': ' '}),
        _event(EventKind.agentTeam, 't4', {
          'v': 1,
          'name': 'Theirs',
        }, author: _other),
      ],
    );
    expect([for (final t in teams) t.name], ['Build', 'Docs']);
    expect(teams.first.description, 'Ships it');
    expect(teams.first.agentPubkeys, [_honey, _fizz]);
    expect(teams.last.description, isNull);
    expect(teams.last.agentPubkeys, isEmpty);
  });
}
