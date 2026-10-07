/// A replaceable speech backend. Offsets are UTF-16 indices into [text].
/// Implementations must finish/cancel the previous utterance before [stop]
/// returns, and must not deliver its callbacks to a subsequent utterance.
abstract interface class SpeechEngine {
  /// Select an installed, offline voice. Throw a user-facing [SpeechFailure]
  /// if speech cannot be provided without a network service.
  Future<void> prepare(String language);

  /// Speak one bounded chunk; true means completed, false means cancelled.
  Future<bool> speak(String text, void Function(int start, int end) onRange);

  /// Cancel and drain the current utterance.
  Future<void> stop();
}

/// An actionable speech error suitable for presentation in the message UI.
class SpeechFailure implements Exception {
  const SpeechFailure(this.message);
  final String message;
  @override
  String toString() => message;
}

/// Selects a local voice without silently falling back to network synthesis.
Map<String, String>? offlineSpeechVoice(
  List<dynamic> voices,
  String language, {
  required bool android,
}) {
  for (final item in voices) {
    if (item is! Map) continue;
    final locale = '${item['locale'] ?? ''}'.replaceAll('_', '-');
    if (locale.split('-').first.toLowerCase() != language.toLowerCase()) {
      continue;
    }
    if (android) {
      final network = item['network_required'];
      if (network != false && network != '0' && network != 0) continue;
      if ('${item['features']}'.contains('notInstalled')) continue;
    }
    final name = item['name'];
    if (name is! String || name.isEmpty) continue;
    return {
      'name': name,
      'locale': '${item['locale']}',
      if (!android && item['identifier'] is String)
        'identifier': item['identifier'] as String,
    };
  }
  return null;
}
