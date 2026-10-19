import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../auth/token/refresh_token_store.dart';
import 'service_address.dart';

/// Done-marker: the service origins whose address-keyed entries were already
/// copied to the service scope. Kept in each store it covers (a string list
/// in SharedPreferences, a JSON list in secure storage) because the keychain
/// survives an app reinstall while preferences do not.
const serviceScopeMigratedKey = 'kuzz.service-scope-migrated.v1';

/// One-time copy of every stored entry whose key embeds a service address
/// (see [ServiceAddresses]) to the same key with that address replaced by
/// the service scope, e.g. `kuzz.auth.refresh.v1:https://host:8443` to
/// `kuzz.auth.refresh.v1:kuzz`.
///
/// - Runs once per service origin: origins listed in the done-marker are
///   skipped, so an entry later deleted under the scoped key (a cleared
///   draft, a sign-out) is never resurrected from the old key.
/// - Copies only when the scoped key is absent and never deletes an old key.
/// - Matches every form the old key builders wrote: `https`/`wss` (or
///   `http`/`ws`), raw or `Uri.encodeComponent`-encoded, explicit default
///   port, an optional trailing slash, any letter case, inside JSON or not.
///   Longer origins are matched first.
/// - An origin is marked done only after its copies succeeded; any failure is
///   logged and retried on the next start. It never throws.
///
/// Must run before anything reads those keys (app start, before `runApp`).
Future<void> migrateServiceScopedStorage({
  required SharedPreferences prefs,
  FlutterSecureStorage? secure,
  ServiceAddresses? addresses,
}) async {
  final service = addresses ?? appServiceAddresses;
  if (service.origins.isEmpty) return;
  try {
    await _migratePrefs(prefs, service);
  } catch (error) {
    debugPrint('[Kuzz] preference scope migration failed: $error');
  }
  try {
    await _migrateSecure(secure ?? const FlutterSecureStorage(), service);
  } catch (error) {
    debugPrint('[Kuzz] secure scope migration failed: $error');
  }
}

/// [key] with every address of [origin] replaced by [scope], or `null` when
/// [key] embeds none. Exposed for tests.
@visibleForTesting
String? serviceScopedKey(String key, String origin, String scope) {
  final pattern = _originPattern(origin);
  if (!pattern.hasMatch(key)) return null;
  return key.replaceAll(pattern, scope);
}

final _patterns = <String, RegExp>{};

RegExp _originPattern(String origin) => _patterns.putIfAbsent(origin, () {
  final uri = Uri.parse(origin);
  final secure = uri.scheme == 'https';
  final schemes = secure ? '(?:https|wss)' : '(?:http|ws)';
  const separator = '(?::|%3A)(?://|%2F%2F)';
  const colon = '(?::|%3A)';
  final host = RegExp.escape(
    uri.host.contains(':') ? '[${uri.host}]' : uri.host,
  ).replaceAll(r'\[', r'(?:\[|%5B)').replaceAll(r'\]', r'(?:\]|%5D)');
  final port = uri.hasPort
      ? '$colon${uri.port}'
      : '(?:$colon${secure ? 443 : 80})?';
  // The address must end here. After an explicit port only another digit
  // would continue it (`host:84430`); without one, more host characters or a
  // port (1-5 digits ending the authority) would (`host.example`,
  // `host:8443`). A `:<hex pubkey>` suffix starting with a digit
  // (`host:8443:3fa9…`) still matches.
  final end = uri.hasPort
      ? r'(?![0-9])(?:/|%2F)?'
      : r'(?:/|%2F)?(?![A-Za-z0-9.\-]|(?::|%3A)\d{1,5}(?![0-9A-Za-z]))';
  return RegExp('$schemes$separator$host$port$end', caseSensitive: false);
});

/// Keys whose old address is a websocket form are processed last, so an
/// up-to-date `https` key wins over a stale pre-canonicalization `wss` one
/// when both map to the same scoped key.
int _websocketLast(String a, String b) {
  int rank(String key) =>
      RegExp(r'wss?(?::|%3A)', caseSensitive: false).hasMatch(key) ? 1 : 0;
  return rank(a).compareTo(rank(b));
}

Future<void> _migratePrefs(
  SharedPreferences prefs,
  ServiceAddresses service,
) async {
  final done = {...?prefs.getStringList(serviceScopeMigratedKey)};
  final pending = service.origins.where((o) => !done.contains(o)).toList();
  if (pending.isEmpty) return;
  for (final origin in pending) {
    final keys =
        prefs.getKeys().where((k) => k != serviceScopeMigratedKey).toList()
          ..sort(_websocketLast);
    for (final key in keys) {
      final target = serviceScopedKey(key, origin, service.scope);
      if (target == null || target == key || prefs.containsKey(target)) {
        continue;
      }
      final value = prefs.get(key);
      final ok = switch (value) {
        final String v => await prefs.setString(target, v),
        final bool v => await prefs.setBool(target, v),
        final int v => await prefs.setInt(target, v),
        final double v => await prefs.setDouble(target, v),
        final List<Object?> v => await prefs.setStringList(
          target,
          v.whereType<String>().toList(),
        ),
        _ => true,
      };
      if (!ok) throw StateError('Could not copy a preference to its scope');
    }
    done.add(origin);
    if (!await prefs.setStringList(serviceScopeMigratedKey, done.toList())) {
      throw StateError('Could not save the scope migration marker');
    }
  }
}

Future<void> _migrateSecure(
  FlutterSecureStorage secure,
  ServiceAddresses service,
) async {
  final done = <String>{};
  final rawDone = await secure.read(key: serviceScopeMigratedKey);
  if (rawDone != null) {
    try {
      final decoded = jsonDecode(rawDone);
      if (decoded is List) done.addAll(decoded.whereType<String>());
    } on FormatException {
      // Unreadable marker: migrate again, copying only absent keys.
    }
  }
  final pending = service.origins.where((o) => !done.contains(o)).toList();
  if (pending.isEmpty) return;
  // iOS filters keychain queries by accessibility, so each class of item is
  // read and copied with the options it was written with. Refresh tokens
  // first: elsewhere readAll returns every item and the default pass then
  // finds their scoped copies already present.
  const passes = <IOSOptions?>[SecureRefreshTokenStore.iOptions, null];
  for (final origin in pending) {
    for (final options in passes) {
      final all = options == null
          ? await secure.readAll()
          : await secure.readAll(iOptions: options);
      final keys = all.keys.where((k) => k != serviceScopeMigratedKey).toList()
        ..sort(_websocketLast);
      for (final key in keys) {
        final target = serviceScopedKey(key, origin, service.scope);
        if (target == null || target == key) continue;
        final existing = options == null
            ? await secure.read(key: target)
            : await secure.read(key: target, iOptions: options);
        if (existing != null || all.containsKey(target)) continue;
        if (options == null) {
          await secure.write(key: target, value: all[key]);
        } else {
          await secure.write(key: target, value: all[key], iOptions: options);
        }
        all[target] = all[key]!;
      }
    }
    done.add(origin);
    await secure.write(
      key: serviceScopeMigratedKey,
      value: jsonEncode(done.toList()),
    );
  }
}
