import 'dart:convert';

import 'package:shared_preferences/shared_preferences.dart';

import 'user_profile.dart';

/// Most profiles kept across launches.
const maxStoredProfiles = 1000;

String _storageKey(String relayBaseUrl) =>
    'kuzz-profile-labels.v1:$relayBaseUrl';

/// The display part of profiles seen on [relayBaseUrl] in earlier sessions:
/// name, avatar URL and handle, so a launch paints known people at once.
/// Never an agent's owner (`ownerPubkey`): identity gates read the relay.
Map<String, UserProfile> readStoredProfiles(
  SharedPreferences prefs,
  String relayBaseUrl,
) {
  try {
    final raw = prefs.getString(_storageKey(relayBaseUrl));
    if (raw == null) return {};
    final decoded = jsonDecode(raw);
    if (decoded is! Map<String, dynamic>) return {};
    final profiles = <String, UserProfile>{};
    for (final entry in decoded.entries) {
      final value = entry.value;
      if (value is! Map<String, dynamic>) continue;
      final pubkey = entry.key.toLowerCase();
      profiles[pubkey] = UserProfile(
        pubkey: pubkey,
        displayName: value['n'] as String?,
        avatarUrl: value['a'] as String?,
        nip05Handle: value['h'] as String?,
      );
    }
    return profiles;
  } catch (_) {
    return {};
  }
}

/// Saves the display part of [profiles] (the last [maxStoredProfiles]).
Future<void> writeStoredProfiles(
  SharedPreferences prefs,
  String relayBaseUrl,
  Map<String, UserProfile> profiles,
) async {
  final entries = profiles.entries.toList();
  final kept = entries.length > maxStoredProfiles
      ? entries.sublist(entries.length - maxStoredProfiles)
      : entries;
  final encoded = <String, Map<String, String>>{
    for (final entry in kept)
      entry.key: {
        if (entry.value.displayName != null) 'n': entry.value.displayName!,
        if (entry.value.avatarUrl != null) 'a': entry.value.avatarUrl!,
        if (entry.value.nip05Handle != null) 'h': entry.value.nip05Handle!,
      },
  };
  try {
    await prefs.setString(_storageKey(relayBaseUrl), jsonEncode(encoded));
  } catch (_) {
    // Storage is a convenience: profiles load from the relay without it.
  }
}
