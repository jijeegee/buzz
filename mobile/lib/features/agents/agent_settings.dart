import 'dart:convert';

import '../../shared/relay/relay.dart';

/// Pure helpers for editing an agent's definition from the phone.
///
/// An owner's agent (kind:30177, `d` = agent pubkey) links to its definition
/// (kind:30175, `d` = slug) through `persona_id`. The phone edits the
/// definition's name, description, instructions and model and republishes it;
/// the agent's computer picks the change up. Rules mirror desktop's
/// `managed_agents/persona_events.rs` and `definition_validation.rs`.

const maxAgentNameChars = 128;
const maxAgentInstructionsBytes = 64 * 1024;
const maxAgentDescriptionChars = 280;

/// The editable fields of an agent definition.
class AgentSettingsDraft {
  const AgentSettingsDraft({
    required this.name,
    required this.description,
    required this.instructions,
    required this.model,
  });

  final String name;
  final String description;
  final String instructions;
  final String model;

  /// Read from a kind:30175 content map.
  factory AgentSettingsDraft.fromContent(Map<String, dynamic> content) =>
      AgentSettingsDraft(
        name: _string(content['display_name']),
        description: _string(content['description']),
        instructions: _string(content['system_prompt']),
        model: _string(content['model']),
      );

  @override
  bool operator ==(Object other) =>
      other is AgentSettingsDraft &&
      other.name == name &&
      other.description == description &&
      other.instructions == instructions &&
      other.model == model;

  @override
  int get hashCode => Object.hash(name, description, instructions, model);
}

String _string(Object? value) => value is String ? value : '';

/// Desktop's `normalize_d_tag`: the kind:30175 `d` tag of a `persona_id`.
String personaDTag(String raw) {
  final buffer = StringBuffer();
  for (final rune in raw.runes) {
    final char = String.fromCharCode(rune);
    final lower = rune < 0x80 ? char.toLowerCase() : char;
    buffer.write(RegExp(r'^[a-z0-9_-]$').hasMatch(lower) ? lower : '-');
  }
  var out = buffer.toString();
  if (out.isEmpty || !RegExp(r'^[a-z0-9]').hasMatch(out)) out = 'a$out';
  return out.length > 64 ? out.substring(0, 64) : out;
}

/// The newest of [events] (NIP-01 replaceable order: later `created_at`,
/// then the lower id), or `null` when there are none.
NostrEvent? newestEvent(Iterable<NostrEvent> events) {
  NostrEvent? newest;
  for (final event in events) {
    if (newest == null ||
        event.createdAt > newest.createdAt ||
        (event.createdAt == newest.createdAt &&
            event.id.compareTo(newest.id) < 0)) {
      newest = event;
    }
  }
  return newest;
}

/// [events] of [kind] by [owner], newest per `d` tag.
Map<String, NostrEvent> newestByDTag(
  Iterable<NostrEvent> events, {
  required int kind,
  required String owner,
}) {
  final ownerLower = owner.toLowerCase();
  final grouped = <String, List<NostrEvent>>{};
  for (final event in events) {
    if (event.kind != kind || event.pubkey.toLowerCase() != ownerLower) {
      continue;
    }
    final d = event.getTagValue('d');
    if (d == null || d.isEmpty) continue;
    (grouped[d] ??= []).add(event);
  }
  return {
    for (final MapEntry(:key, :value) in grouped.entries)
      key: newestEvent(value)!,
  };
}

/// The JSON object in [content], or `null` when it is not one.
Map<String, dynamic>? decodeJsonObject(String content) {
  try {
    final decoded = jsonDecode(content);
    return decoded is Map<String, dynamic> ? decoded : null;
  } on FormatException {
    return null;
  }
}

/// The `persona_id` of a kind:30177 [content], if any.
String? managedAgentPersonaId(String content) {
  final value = decodeJsonObject(content)?['persona_id'];
  return value is String && value.trim().isNotEmpty ? value : null;
}

/// Why [draft] cannot be saved, or `null` when it can.
String? validateAgentSettings(AgentSettingsDraft draft) {
  final name = draft.name.trim();
  if (name.isEmpty) return 'Name is required.';
  final nameChars = name.runes.length;
  if (nameChars > maxAgentNameChars) {
    return 'Name is too long ($nameChars characters, max $maxAgentNameChars).';
  }
  final description = draft.description;
  final descriptionChars = description.runes.length;
  if (descriptionChars > maxAgentDescriptionChars) {
    return 'Description is too long ($descriptionChars characters, max '
        '$maxAgentDescriptionChars).';
  }
  final instructionsBytes = utf8.encode(draft.instructions).length;
  if (instructionsBytes > maxAgentInstructionsBytes) {
    return 'Instructions are too long ($instructionsBytes bytes, max '
        '$maxAgentInstructionsBytes).';
  }
  return _invisibleCharacter(name, 'Name') ??
      // Dart's trim also drops edge U+FEFF, which desktop's trim keeps and
      // then rejects; check the untrimmed name for it too.
      _invisibleCharacter(draft.name, 'Name', ignorableOnly: true) ??
      _invisibleCharacter(description, 'Description') ??
      _invisibleCharacter(
        draft.instructions,
        'Instructions',
        allowLayout: true,
      ) ??
      _modelError(draft.model.trim());
}

String? _modelError(String model) {
  for (final rune in model.runes) {
    if (_isControl(rune) || _isDefaultIgnorable(rune)) {
      return 'Model contains an invisible or formatting character.';
    }
  }
  return null;
}

/// Desktop's `validate_visible_text`: control and default-ignorable
/// characters are refused, except emoji VS16 / ZWJ inside an emoji sequence
/// and, when [allowLayout], `\n` and `\t`.
String? _invisibleCharacter(
  String value,
  String label, {
  bool allowLayout = false,
  bool ignorableOnly = false,
}) {
  final runes = value.runes.toList();
  for (var index = 0; index < runes.length; index++) {
    final rune = runes[index];
    final layout =
        ignorableOnly || (allowLayout && (rune == 0x0A || rune == 0x09));
    if ((!layout && _isControl(rune)) ||
        (_isDefaultIgnorable(rune) && !_isAllowedEmojiFormat(runes, index))) {
      final code = rune.toRadixString(16).toUpperCase().padLeft(4, '0');
      return '$label contains an invisible or formatting character '
          '(U+$code).';
    }
  }
  return null;
}

const _variationSelector16 = 0xFE0F;
const _zeroWidthJoiner = 0x200D;

final _extendedPictographic = RegExp(
  r'^\p{Extended_Pictographic}$',
  unicode: true,
);

bool _isAllowedEmojiFormat(List<int> runes, int index) {
  switch (runes[index]) {
    case _variationSelector16:
      return index > 0 && _isEmojiVariationBase(runes[index - 1]);
    case _zeroWidthJoiner:
      return _hasPrecedingEmojiBase(runes, index) &&
          index + 1 < runes.length &&
          _isExtendedPictographic(runes[index + 1]);
    default:
      return false;
  }
}

bool _hasPrecedingEmojiBase(List<int> runes, int index) {
  for (var previous = index - 1; previous >= 0; previous--) {
    final rune = runes[previous];
    if (rune != _variationSelector16 && !_isEmojiModifier(rune)) {
      return _isExtendedPictographic(rune);
    }
  }
  return false;
}

bool _isEmojiVariationBase(int rune) =>
    rune == 0x23 ||
    rune == 0x2A ||
    (rune >= 0x30 && rune <= 0x39) ||
    _isExtendedPictographic(rune);

bool _isEmojiModifier(int rune) => rune >= 0x1F3FB && rune <= 0x1F3FF;

bool _isExtendedPictographic(int rune) =>
    _extendedPictographic.hasMatch(String.fromCharCode(rune));

/// Rust `char::is_control` (general category Cc).
bool _isControl(int rune) => rune <= 0x1F || (rune >= 0x7F && rune <= 0x9F);

/// Unicode `Default_Ignorable_Code_Point`, as in desktop.
bool _isDefaultIgnorable(int rune) =>
    rune == 0x00AD ||
    rune == 0x034F ||
    rune == 0x061C ||
    (rune >= 0x115F && rune <= 0x1160) ||
    (rune >= 0x17B4 && rune <= 0x17B5) ||
    (rune >= 0x180B && rune <= 0x180F) ||
    (rune >= 0x200B && rune <= 0x200F) ||
    (rune >= 0x202A && rune <= 0x202E) ||
    (rune >= 0x2060 && rune <= 0x206F) ||
    rune == 0x3164 ||
    (rune >= 0xFE00 && rune <= 0xFE0F) ||
    rune == 0xFEFF ||
    rune == 0xFFA0 ||
    (rune >= 0xFFF0 && rune <= 0xFFF8) ||
    (rune >= 0x1BCA0 && rune <= 0x1BCA3) ||
    (rune >= 0x1D173 && rune <= 0x1D17A) ||
    (rune >= 0xE0000 && rune <= 0xE0FFF);

/// Desktop's kind:30175 field order (`PersonaEventContent`); other keys
/// follow in their original order.
const _personaFieldOrder = [
  'display_name',
  'system_prompt',
  'acp_command',
  'avatar_url',
  'runtime',
  'model',
  'provider',
  'name_pool',
  'respond_to',
  'respond_to_allowlist',
  'parallelism',
  'description',
  'session_policy',
  'effort_level',
];

/// [loaded] kind:30175 content with [draft] applied, as desktop writes it:
/// trimmed name; instructions verbatim; description and model trimmed and
/// left out when blank. Every other key is kept.
String patchedPersonaContent(
  Map<String, dynamic> loaded,
  AgentSettingsDraft draft,
) {
  final patched = Map<String, dynamic>.of(loaded)
    ..['display_name'] = draft.name.trim();
  // Desktop writes the prompt whenever the definition has one, even empty.
  if (draft.instructions.isNotEmpty || loaded.containsKey('system_prompt')) {
    patched['system_prompt'] = draft.instructions;
  }
  for (final (key, value) in [
    ('description', draft.description.trim()),
    ('model', draft.model.trim()),
  ]) {
    if (value.isEmpty) {
      patched.remove(key);
    } else {
      patched[key] = value;
    }
  }
  final ordered = <String, dynamic>{
    for (final key in _personaFieldOrder)
      if (patched.containsKey(key)) key: patched[key],
  };
  for (final MapEntry(:key, :value) in patched.entries) {
    ordered.putIfAbsent(key, () => value);
  }
  return jsonEncode(ordered);
}

/// A copy of [tags] for the republished event (same `d`, `shared`, …).
List<List<String>> copyTags(List<List<String>> tags) => [
  for (final tag in tags) [...tag],
];

/// NIP-AP monotonic `created_at`: `max(now, previous + 1)`.
int nextCreatedAt({required int previous, required int now}) =>
    now > previous ? now : previous + 1;

String _nameKey(String name) => name.trim().toLowerCase();

/// Whether another of the owner's agents or definitions is already named
/// [name] (trimmed, case-insensitive). The agent [agentPubkey] itself, that
/// definition [personaD], and other agents of it that still carry its
/// [previousName] (its computer renames them along with it) are not
/// conflicts.
bool agentNameConflicts({
  required String name,
  required String previousName,
  required String owner,
  required String agentPubkey,
  required String personaD,
  required Iterable<NostrEvent> agentEvents,
  required Iterable<NostrEvent> personaEvents,
}) {
  final wanted = _nameKey(name);
  if (wanted.isEmpty) return false;
  final agents = newestByDTag(
    agentEvents,
    kind: EventKind.managedAgent,
    owner: owner,
  );
  for (final MapEntry(key: d, value: event) in agents.entries) {
    if (d.toLowerCase() == agentPubkey.toLowerCase()) continue;
    final content = decodeJsonObject(event.content);
    if (content == null) continue;
    final personaId = content['persona_id'];
    final agentName = _string(content['name']);
    if (personaId is String &&
        personaId.trim().isNotEmpty &&
        personaDTag(personaId) == personaD &&
        agentName == previousName) {
      continue;
    }
    if (_nameKey(agentName) == wanted) return true;
  }
  final personas = newestByDTag(
    personaEvents,
    kind: EventKind.persona,
    owner: owner,
  );
  for (final MapEntry(key: d, value: event) in personas.entries) {
    if (d == personaD) continue;
    final content = decodeJsonObject(event.content);
    if (content == null) continue;
    if (_nameKey(_string(content['display_name'])) == wanted) return true;
  }
  return false;
}

/// The name-conflict error for [name].
String agentNameConflictMessage(String name) =>
    'You already have an agent named "${name.trim()}". '
    'Choose a different name.';
