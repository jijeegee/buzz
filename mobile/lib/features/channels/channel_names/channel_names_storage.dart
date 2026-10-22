import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// Kind 30078 `d` tag of the private chat names ("Rename for me").
const channelNamesDTag = 'channel-names';

/// Longest private name kept; longer input is cut.
const channelNameMaxLength = 64;

String channelNamesKey(String pubkey) => 'kuzz.channel-names.v1:$pubkey';

/// One private name, or a restore ([name] null), stamped when it was set.
@immutable
class ChannelNameEntry {
  const ChannelNameEntry({required this.name, required this.updatedAt});

  /// Null after Restore name.
  final String? name;
  final int updatedAt;

  Map<String, dynamic> toJson() => {'name': name, 'updatedAt': updatedAt};

  @override
  bool operator ==(Object other) =>
      other is ChannelNameEntry &&
      other.name == name &&
      other.updatedAt == updatedAt;

  @override
  int get hashCode => Object.hash(name, updatedAt);
}

/// This device's private names by channel id.
@immutable
class ChannelNameStore {
  const ChannelNameStore({this.channels = const {}});

  final Map<String, ChannelNameEntry> channels;

  /// The private name for [channelId], or null when none is set.
  String? nameFor(String channelId) => channels[channelId]?.name;

  /// The synced document: `{"<channel_id>":"name"}`, set names only.
  Map<String, String> toRemote() => {
    for (final entry in channels.entries)
      if (entry.value.name != null) entry.key: entry.value.name!,
  };

  Map<String, dynamic> toJson() => {
    'version': 1,
    'channels': {
      for (final entry in channels.entries) entry.key: entry.value.toJson(),
    },
  };

  factory ChannelNameStore.fromJson(Map<String, dynamic> json) {
    final raw = json['channels'];
    final channels = <String, ChannelNameEntry>{};
    if (raw is Map) {
      for (final entry in raw.entries) {
        final value = entry.value;
        if (entry.key is! String || value is! Map) continue;
        final name = value['name'];
        final updatedAt = value['updatedAt'];
        if (updatedAt is! int || (name != null && name is! String)) continue;
        channels[entry.key as String] = ChannelNameEntry(
          name: name as String?,
          updatedAt: updatedAt,
        );
      }
    }
    return ChannelNameStore(channels: channels);
  }
}

/// A private name as stored: trimmed, cut to [channelNameMaxLength], and
/// null when blank (a blank name restores the shared one).
String? normalizeChannelName(String? name) {
  final trimmed = name?.trim() ?? '';
  if (trimmed.isEmpty) return null;
  final runes = trimmed.runes.toList();
  return runes.length <= channelNameMaxLength
      ? trimmed
      : String.fromCharCodes(runes.take(channelNameMaxLength)).trim();
}

/// Parses a synced document; null when it is not a JSON object.
Map<String, String>? parseRemoteChannelNames(String plaintext) {
  final Object? parsed;
  try {
    parsed = jsonDecode(plaintext);
  } on FormatException {
    return null;
  }
  if (parsed is! Map) return null;
  final names = <String, String>{};
  for (final entry in parsed.entries) {
    final key = entry.key;
    final value = entry.value;
    if (key is! String || value is! String) continue;
    final name = normalizeChannelName(value);
    if (name != null) names[key] = name;
  }
  return names;
}

/// Applies a synced document written at [remoteAt] (last write wins per
/// chat): every chat it names takes that name and every chat it leaves out
/// is restored, unless this device changed it after [remoteAt].
ChannelNameStore mergeRemoteChannelNames(
  ChannelNameStore local,
  Map<String, String> remote,
  int remoteAt,
) {
  final merged = <String, ChannelNameEntry>{...local.channels};
  for (final id in {...local.channels.keys, ...remote.keys}) {
    final current = local.channels[id];
    if (current != null && current.updatedAt > remoteAt) continue;
    final name = remote[id];
    if (current?.name == name) continue;
    merged[id] = ChannelNameEntry(name: name, updatedAt: remoteAt);
  }
  return ChannelNameStore(channels: merged);
}

class ChannelNamesStorage {
  ChannelNamesStorage(this._prefs);

  final SharedPreferences _prefs;

  ChannelNameStore read(String pubkey) {
    final raw = _prefs.getString(channelNamesKey(pubkey));
    if (raw == null || raw.isEmpty) return const ChannelNameStore();
    try {
      final parsed = jsonDecode(raw);
      if (parsed is! Map<String, dynamic> || parsed['version'] != 1) {
        return const ChannelNameStore();
      }
      return ChannelNameStore.fromJson(parsed);
    } catch (_) {
      return const ChannelNameStore();
    }
  }

  Future<bool> write(String pubkey, ChannelNameStore store) =>
      _prefs.setString(channelNamesKey(pubkey), jsonEncode(store.toJson()));
}
