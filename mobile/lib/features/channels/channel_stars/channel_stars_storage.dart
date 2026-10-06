import 'dart:convert';

import 'package:shared_preferences/shared_preferences.dart';

import '../../../shared/auth/token/relay_origin.dart';

/// Durable personal pins are isolated by signer and normalized community.
String channelStarsKey(String pubkey, {String? origin}) => origin == null
    ? 'buzz.channel-stars.v1:$pubkey'
    : 'buzz.channel-stars.v2:${jsonEncode([pubkey, normalizeRelayOrigin(origin)])}';

class ChannelStarEntry {
  final bool starred;
  final int updatedAt;

  const ChannelStarEntry({required this.starred, required this.updatedAt});

  Map<String, dynamic> toJson() => {'starred': starred, 'updatedAt': updatedAt};

  factory ChannelStarEntry.fromJson(Map<String, dynamic> json) =>
      ChannelStarEntry(
        starred: json['starred'] as bool,
        updatedAt: json['updatedAt'] as int,
      );
}

class ChannelStarStore {
  final int version;
  final Map<String, ChannelStarEntry> channels;

  const ChannelStarStore({this.version = 1, this.channels = const {}});

  Map<String, dynamic> toJson() => {
    'version': version,
    'channels': {for (final e in channels.entries) e.key: e.value.toJson()},
  };

  factory ChannelStarStore.fromJson(Map<String, dynamic> json) {
    final rawChannels = json['channels'];
    final channels = <String, ChannelStarEntry>{};
    if (rawChannels is Map) {
      for (final entry in rawChannels.entries) {
        if (entry.key is String && entry.value is Map<String, dynamic>) {
          final v = entry.value as Map<String, dynamic>;
          if (v['starred'] is bool && v['updatedAt'] is int) {
            channels[entry.key as String] = ChannelStarEntry.fromJson(v);
          }
        }
      }
    }
    return ChannelStarStore(version: 1, channels: channels);
  }
}

ChannelStarStore mergeStores(ChannelStarStore local, ChannelStarStore remote) {
  // Per-channel max-updatedAt merge:
  // For each channel ID in the union, keep the entry with the highest updatedAt.
  final merged = <String, ChannelStarEntry>{...local.channels};
  for (final entry in remote.channels.entries) {
    final existing = merged[entry.key];
    if (existing == null || entry.value.updatedAt > existing.updatedAt) {
      merged[entry.key] = entry.value;
    }
  }
  return ChannelStarStore(channels: merged);
}

class ChannelStarsStorage {
  final SharedPreferences _prefs;

  ChannelStarsStorage(this._prefs);

  ChannelStarStore read(String pubkey, {String? origin}) {
    final raw = _prefs.getString(channelStarsKey(pubkey, origin: origin));
    if (raw == null || raw.isEmpty) {
      return const ChannelStarStore();
    }

    try {
      final parsed = jsonDecode(raw);
      if (parsed is! Map<String, dynamic>) {
        return const ChannelStarStore();
      }
      if (parsed['version'] != 1) {
        return const ChannelStarStore();
      }
      return ChannelStarStore.fromJson(parsed);
    } catch (_) {
      return const ChannelStarStore();
    }
  }

  /// Claim unscoped legacy data for exactly one community. Keep the source
  /// intact, including after an interrupted/failed copy, so retry is safe.
  Future<ChannelStarStore> load(String pubkey, {String? origin}) async {
    if (origin == null) return read(pubkey);
    final canonical = normalizeRelayOrigin(origin);
    final key = channelStarsKey(pubkey, origin: canonical);
    if (_prefs.containsKey(key)) return read(pubkey, origin: canonical);
    final legacy = _prefs.getString(channelStarsKey(pubkey));
    if (legacy == null) return const ChannelStarStore();
    final claim = 'buzz.channel-stars.migrated-origin:$pubkey';
    final owner = _prefs.getString(claim);
    if (owner != null && owner != canonical) return const ChannelStarStore();
    if (owner == null && !await _prefs.setString(claim, canonical)) {
      throw StateError('Could not preserve legacy pin scope');
    }
    if (!await _prefs.setString(key, legacy)) {
      throw StateError('Could not persist scoped pins');
    }
    return read(pubkey, origin: canonical);
  }

  Future<bool> write(String pubkey, ChannelStarStore store, {String? origin}) =>
      _prefs.setString(
        channelStarsKey(pubkey, origin: origin),
        jsonEncode(store.toJson()),
      );
}
