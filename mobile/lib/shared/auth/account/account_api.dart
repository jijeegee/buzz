import 'dart:convert';

import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;

import '../../community/community.dart';
import '../../community/community_provider.dart';
import '../token/token.dart';
import '../../l10n/l10n.dart';

/// A failed account request (`/auth/me`, `/auth/profile`, `/auth/devices`,
/// `/auth/sessions/*`, `/auth/bots/*`, `/auth/account`, `/auth/blocks`,
/// `/auth/people/*`).
class AccountApiException implements Exception {
  const AccountApiException(this.message, {this.statusCode, this.code});

  /// Human-readable reason (relay `error` field when present).
  final String message;

  /// HTTP status, or `null` when no response arrived.
  final int? statusCode;

  /// Relay machine code (`username_taken`, `token_expired`, ...).
  final String? code;

  @override
  String toString() => message;
}

/// One agent's owner tools (`POST /auth/agents/tool-levels`): `full` (on),
/// `limited` (off), or `null` when never set, which acts as off.
class AgentToolLevel {
  const AgentToolLevel({
    required this.pubkey,
    this.level,
    this.respondClass,
    this.here = false,
  });

  final String pubkey;
  final String? level;

  /// Who the agent answers, as the relay last stored it.
  final String? respondClass;

  /// Whether the calling device hosts the agent.
  final bool here;

  bool get on => level == 'full';

  static AgentToolLevel? fromJson(Map<String, Object?> json) {
    final pubkey = json['pubkey'];
    if (pubkey is! String || pubkey.isEmpty) return null;
    final level = json['level'];
    final respondClass = json['respond_class'];
    return AgentToolLevel(
      pubkey: pubkey.toLowerCase(),
      level: level is String ? level : null,
      respondClass: respondClass is String ? respondClass : null,
      here: json['here'] == true,
    );
  }
}

/// The signed-in principal's global profile (`GET /auth/me`,
/// `PATCH /auth/profile`).
class AccountProfile {
  const AccountProfile({
    required this.principalId,
    required this.displayName,
    this.avatarUrl,
    this.username,
    this.email,
    this.discoverable = true,
    this.locale,
    this.deviceKey,
  });

  factory AccountProfile.fromJson(Map<String, Object?> json) {
    final principal = json['principal_id'];
    if (principal is! String || principal.isEmpty) {
      throw const AccountApiException(
        'Malformed relay response: missing principal_id',
      );
    }
    final name = json['display_name'];
    final avatar = json['avatar_url'];
    final username = json['username'];
    final email = json['email'];
    final discoverable = json['discoverable'];
    final locale = json['locale'];
    final deviceKey = json['device_key'];
    return AccountProfile(
      principalId: principal,
      displayName: name is String ? name : '',
      avatarUrl: avatar is String && avatar.isNotEmpty ? avatar : null,
      username: username is String && username.isNotEmpty ? username : null,
      email: email is String && email.isNotEmpty ? email : null,
      // Relays before people lookup send none; the account default is on.
      discoverable: discoverable is bool ? discoverable : true,
      locale: locale is String && locale.isNotEmpty ? locale : null,
      deviceKey: deviceKey is bool ? deviceKey : null,
    );
  }

  final String principalId;
  final String displayName;
  final String? avatarUrl;
  final String? username;

  /// The Google email of this session's account; relays before it was
  /// added to `/auth/me` send none.
  final String? email;

  /// Whether other people can find this account by its exact Kuzz ID or
  /// email (`GET /auth/people/lookup`).
  final bool discoverable;

  /// The app language saved on the account (`ko`, `zh-Hans`); server text
  /// such as moderation notices uses it. Older relays send none.
  final String? locale;

  /// Whether this person session's device has a hardware device key.
  /// `false` means it signed in before device keys and must sign in again;
  /// `null` for bots and relays that do not say.
  final bool? deviceKey;
}

/// Another person as the people endpoints return them
/// (`/auth/people/lookup`, `/auth/people/shared`). Never carries an email.
class PersonSummary {
  const PersonSummary({
    required this.pubkey,
    this.displayName,
    this.avatarUrl,
    this.username,
  });

  factory PersonSummary.fromJson(Map<String, Object?> json) {
    final pubkey = json['pubkey'];
    if (pubkey is! String || !_hexPubkey.hasMatch(pubkey.toLowerCase())) {
      throw const AccountApiException('Malformed relay response: pubkey');
    }
    final name = json['display_name'];
    final avatar = json['avatar_url'];
    final username = json['username'];
    return PersonSummary(
      pubkey: pubkey.toLowerCase(),
      displayName: name is String && name.trim().isNotEmpty ? name : null,
      avatarUrl: avatar is String && avatar.isNotEmpty ? avatar : null,
      username: username is String && username.isNotEmpty ? username : null,
    );
  }

  /// Lowercase hex public key.
  final String pubkey;
  final String? displayName;
  final String? avatarUrl;

  /// The person's Kuzz ID, when they set one.
  final String? username;
}

/// One person this account blocked (`GET /auth/blocks`).
class BlockedPerson {
  const BlockedPerson({required this.pubkey, this.createdAt});

  final String pubkey;
  final DateTime? createdAt;
}

final _hexPubkey = RegExp(r'^[0-9a-f]{64}$');

/// Lowercase [pubkey]; throws [ArgumentError] unless it is 64 hex
/// characters, so a malformed key never reaches a request path.
String _pathPubkey(String pubkey) {
  final normalized = pubkey.trim().toLowerCase();
  if (!_hexPubkey.hasMatch(normalized)) {
    throw ArgumentError.value(pubkey, 'pubkey', 'not a hex public key');
  }
  return normalized;
}

/// An agent connected on an external device.
class AccountDeviceAgent {
  const AccountDeviceAgent({required this.pubkey, this.runtime});

  final String pubkey;

  /// What it runs on (`Hermes`), when the connection said.
  final String? runtime;
}

/// One signed-in device (`GET /auth/devices`).
class AccountDevice {
  const AccountDevice({
    required this.id,
    required this.name,
    required this.platform,
    required this.current,
    this.lastSeenAt,
    this.loginDeviceId,
    this.agents = const [],
  });

  factory AccountDevice.fromJson(Map<String, Object?> json) {
    final id = json['id'];
    if (id is! String || id.isEmpty) {
      throw const AccountApiException('Malformed relay response: device id');
    }
    final name = json['name'];
    final platform = json['platform'];
    final lastSeen = json['last_seen_at'];
    final loginDevice = json['login_device_id'];
    final agents = json['agents'];
    return AccountDevice(
      id: id,
      name: name is String && name.isNotEmpty
          ? name
          : appL10n.accountUnnamedDevice,
      platform: platform is String ? platform : '',
      current: json['current'] == true,
      lastSeenAt: lastSeen is String ? DateTime.tryParse(lastSeen) : null,
      loginDeviceId: loginDevice is String ? loginDevice : null,
      agents: [
        if (agents is List)
          for (final agent in agents)
            if (agent is Map && agent['pubkey'] is String)
              AccountDeviceAgent(
                pubkey: agent['pubkey'] as String,
                runtime: switch (agent['runtime']) {
                  final String runtime when runtime.trim().isNotEmpty =>
                    runtime.trim(),
                  _ => null,
                },
              ),
      ],
    );
  }

  final String id;
  final String name;

  /// `desktop`, `mobile`, `web`, `cli` or `external`.
  final String platform;

  /// Whether this is the device making the request.
  final bool current;
  final DateTime? lastSeenAt;

  /// An external device's computer, when signed in there.
  final String? loginDeviceId;

  /// The agents connected on an external device.
  final List<AccountDeviceAgent> agents;

  bool get isExternal => platform == 'external';
}

/// The account endpoints of one token relay, authenticated with the
/// session's Bearer access token.
class AccountApi {
  AccountApi({
    required this.origin,
    required http.Client client,
    required TokenSessionController session,
    this.timeout = const Duration(seconds: 20),
  }) : _client = client,
       _session = session;

  final String origin;
  final http.Client _client;
  final TokenSessionController _session;
  final Duration timeout;

  Future<AccountProfile> me() async {
    final response = await _send('GET', '/auth/me');
    return AccountProfile.fromJson(_object(response));
  }

  /// Save the whole profile in one request: a single atomic write on the
  /// relay. `null` [avatarUrl]/[username] clear the value.
  Future<AccountProfile> updateProfile({
    required String displayName,
    required String? avatarUrl,
    required String? username,
  }) async {
    final response = await _send(
      'PATCH',
      '/auth/profile',
      body: {
        'display_name': displayName,
        'avatar_url': avatarUrl,
        'username': username,
      },
    );
    return AccountProfile.fromJson(_object(response));
  }

  /// Set or clear (`null`) the Kuzz ID alone; other profile fields keep
  /// their value. A taken ID fails with code `username_taken` (409).
  Future<AccountProfile> setUsername(String? username) async {
    final response = await _send(
      'PATCH',
      '/auth/profile',
      body: {'username': username},
    );
    return AccountProfile.fromJson(_object(response));
  }

  /// Whether people can find this account by exact Kuzz ID or email.
  Future<void> setDiscoverable(bool discoverable) =>
      _send('PATCH', '/auth/profile', body: {'discoverable': discoverable});

  /// Save the app language (`ko`, `zh-Hans`) on the account.
  Future<void> setLocale(String locale) =>
      _send('PATCH', '/auth/profile', body: {'locale': locale});

  /// The people this account blocked; only the account itself can read it.
  Future<List<BlockedPerson>> listBlocks() async {
    final response = await _send('GET', '/auth/blocks');
    final blocked = _object(response)['blocked'];
    if (blocked is! List) {
      throw const AccountApiException('Malformed relay response: blocked');
    }
    final result = <BlockedPerson>[];
    for (final item in blocked) {
      if (item is! Map<String, Object?>) continue;
      final pubkey = item['pubkey'];
      if (pubkey is! String || !_hexPubkey.hasMatch(pubkey.toLowerCase())) {
        continue;
      }
      final createdAt = item['created_at'];
      result.add(
        BlockedPerson(
          pubkey: pubkey.toLowerCase(),
          createdAt: createdAt is String ? DateTime.tryParse(createdAt) : null,
        ),
      );
    }
    return result;
  }

  /// Block [pubkey] (idempotent).
  Future<void> block(String pubkey) =>
      _send('PUT', '/auth/blocks/${_pathPubkey(pubkey)}');

  /// Unblock [pubkey] (idempotent).
  Future<void> unblock(String pubkey) =>
      _send('DELETE', '/auth/blocks/${_pathPubkey(pubkey)}');

  /// The one discoverable person whose Kuzz ID or email is exactly [query],
  /// or `null` when there is none (404). Too many lookups fail with 429.
  /// A leading `@` on an ID ("@ada") is dropped: the relay reads any `@` as
  /// an email address.
  Future<PersonSummary?> lookupPerson(String query) async {
    var q = query.trim();
    if (q.startsWith('@') && !q.substring(1).contains('@')) {
      q = q.substring(1);
    }
    final http.Response response;
    try {
      response = await _send('GET', '/auth/people/lookup', query: {'q': q});
    } on AccountApiException catch (error) {
      if (error.statusCode == 404) return null;
      rethrow;
    }
    return PersonSummary.fromJson(_object(response));
  }

  /// People in this account's channels whose name or Kuzz ID starts with
  /// [query]; at most [limit], which the relay caps at 50.
  Future<List<PersonSummary>> sharedPeople(
    String query, {
    int limit = 50,
  }) async {
    final response = await _send(
      'GET',
      '/auth/people/shared',
      query: {'q': query.trim(), 'limit': '${limit.clamp(1, 50)}'},
    );
    final people = _object(response)['people'];
    if (people is! List) {
      throw const AccountApiException('Malformed relay response: people');
    }
    final result = <PersonSummary>[];
    for (final item in people) {
      if (item is! Map<String, Object?>) continue;
      try {
        result.add(PersonSummary.fromJson(item));
      } on AccountApiException {
        continue;
      }
    }
    return result;
  }

  Future<List<AccountDevice>> listDevices() async {
    final response = await _send('GET', '/auth/devices');
    final decoded = _decode(response.body);
    if (decoded is! List) {
      throw const AccountApiException('Malformed relay response: devices');
    }
    return [
      for (final item in decoded)
        if (item is Map<String, Object?>) AccountDevice.fromJson(item),
    ];
  }

  /// Sign [deviceId] out remotely (revokes its session).
  Future<void> revokeDevice(String deviceId) =>
      _send('DELETE', '/auth/devices/${Uri.encodeComponent(deviceId)}');

  /// Take agent [pubkey] off external device [deviceId]. Its last agent takes
  /// the device with it.
  Future<void> disconnectAgent(String deviceId, String pubkey) => _send(
    'DELETE',
    '/auth/devices/${Uri.encodeComponent(deviceId)}'
        '/agents/${Uri.encodeComponent(pubkey)}',
  );

  /// Set external agent [pubkey]'s name, picture and description, as
  /// absolute state: a null [avatarUrl] or [about] clears it.
  Future<void> setExternalAgentProfile(
    String pubkey, {
    required String displayName,
    required String? avatarUrl,
    required String? about,
  }) => _send(
    'PUT',
    '/auth/agents/${Uri.encodeComponent(pubkey.toLowerCase())}/profile',
    body: {
      'display_name': displayName,
      'avatar_url': avatarUrl,
      'about': about,
    },
  );

  /// Rename [deviceId] (any of this account's devices).
  Future<AccountDevice> renameDevice(String deviceId, String name) async {
    final response = await _send(
      'PATCH',
      '/auth/devices/${Uri.encodeComponent(deviceId)}',
      body: {'name': name},
    );
    return AccountDevice.fromJson(_object(response));
  }

  /// Set agent [pubkey]'s owner tools: [level] `full` or `limited`, with the
  /// class it answers ([respondClass]). Returns `set` or `already_set`.
  /// Refused with 409 `full_anyone_refused` or `respond_class_stale`.
  Future<String> setToolLevel(
    String pubkey, {
    required String level,
    required String respondClass,
    bool isDefault = false,
  }) async {
    final response = await _send(
      'POST',
      '/auth/agents/${Uri.encodeComponent(pubkey.toLowerCase())}/tool-level',
      body: {
        'level': level,
        'respond_class': respondClass,
        'default': isDefault,
      },
    );
    final result = _object(response)['result'];
    return result is String ? result : 'set';
  }

  /// Owner tools of [pubkeys] (batches of 64), by lowercase key. A key
  /// missing from the answer is unknown, read as off.
  Future<Map<String, AgentToolLevel>> toolLevels(List<String> pubkeys) async {
    final levels = <String, AgentToolLevel>{};
    for (var start = 0; start < pubkeys.length; start += 64) {
      final batch = pubkeys.sublist(
        start,
        start + 64 > pubkeys.length ? pubkeys.length : start + 64,
      );
      final response = await _send(
        'POST',
        '/auth/agents/tool-levels',
        body: {
          'pubkeys': [for (final key in batch) key.toLowerCase()],
        },
      );
      final agents = _object(response)['agents'];
      if (agents is! List) {
        throw const AccountApiException('Malformed relay response: agents');
      }
      for (final item in agents) {
        if (item is! Map<String, Object?>) continue;
        final level = AgentToolLevel.fromJson(item);
        if (level != null) levels[level.pubkey] = level;
      }
    }
    return levels;
  }

  /// Sign out every other human session; bots are untouched.
  Future<void> revokeOtherSessions() =>
      _send('POST', '/auth/sessions/revoke-others');

  Future<void> revokeAllBotTokens() => _send('POST', '/auth/bots/revoke-all');

  /// Disable the account now (purged after 30 days unless signed in again).
  /// Also ends this session on the relay.
  Future<void> deleteAccount() => _send('DELETE', '/auth/account');

  Future<http.Response> _send(
    String method,
    String path, {
    Map<String, Object?>? body,
    Map<String, String>? query,
  }) async {
    // A session nobody has restored yet (the gate normally does) has no
    // refresh token in memory: restore it rather than report signed out.
    if (_session.state.status.isRestoring) {
      await _session.restore();
    }
    var token = await _session.ensureFreshAccessToken();
    if (token == null) throw _signedOut();
    var response = await _request(method, path, token, body, query);
    if (response.statusCode == 401 && _code(response) == 'token_expired') {
      token = await _session.refreshAfterTokenExpired(token);
      if (token == null) throw _signedOut();
      response = await _request(method, path, token, body, query);
    }
    final status = response.statusCode;
    if (status >= 200 && status < 300) return response;
    final code = _code(response);
    if (code == 'token_revoked' ||
        code == 'principal_disabled' ||
        code == 'device_key_required') {
      // The session is dead on the relay. Presenting the refresh token lets
      // the session controller learn that (a terminal refresh signs it out
      // and the gate shows sign-in) instead of staying "signed in" until the
      // next scheduled rotation.
      await _session.refreshAfterTokenExpired(token);
    }
    final error = _decode(response.body);
    final message = error is Map<String, Object?> ? error['error'] : null;
    throw AccountApiException(
      message is String && message.isNotEmpty ? message : 'HTTP $status',
      statusCode: status,
      code: code,
    );
  }

  Future<http.Response> _request(
    String method,
    String path,
    String token,
    Map<String, Object?>? body,
    Map<String, String>? query,
  ) async {
    final uri = Uri.parse('$origin$path');
    final request = http.Request(
      method,
      query == null ? uri : uri.replace(queryParameters: query),
    )..headers['Authorization'] = 'Bearer $token';
    if (body != null) {
      request.headers['Content-Type'] = 'application/json';
      request.body = jsonEncode(body);
    }
    try {
      final streamed = await _client.send(request).timeout(timeout);
      return await http.Response.fromStream(streamed).timeout(timeout);
    } catch (error) {
      throw AccountApiException(appL10n.accountRelayUnreachable('$error'));
    }
  }

  static AccountApiException _signedOut() =>
      AccountApiException(appL10n.accountSignedOut);

  static String? _code(http.Response response) {
    final decoded = _decode(response.body);
    final code = decoded is Map<String, Object?> ? decoded['code'] : null;
    return code is String ? code : null;
  }

  static Map<String, Object?> _object(http.Response response) {
    final decoded = _decode(response.body);
    if (decoded is Map<String, Object?>) return decoded;
    throw const AccountApiException('Malformed relay response');
  }

  static Object? _decode(String body) {
    if (body.isEmpty) return null;
    try {
      return jsonDecode(body);
    } on FormatException {
      return null;
    }
  }
}

/// Whether the account [key] names is a Google-held key account. Asked of
/// its row, not of the active community: a caller may name an account that is
/// no longer the active one.
final _keyBackupAccountProvider = Provider.family<bool, SessionKey>((ref, key) {
  if (ref.watch(activeGoogleKeySessionProvider) == key) return true;
  final rows = ref.watch(communityListProvider).value ?? const <Community>[];
  return rows.any(
    (row) => !row.tokenAuth && SessionKey.ofCommunity(row) == key,
  );
});

/// Account endpoints of the account [SessionKey] names.
final accountApiProvider = Provider.family<AccountApi, SessionKey>((ref, key) {
  final googleKey = ref.watch(_keyBackupAccountProvider(key));
  return AccountApi(
    origin: key.origin,
    client: ref.watch(authHttpClientProvider),
    session: ref.watch(
      googleKey
          ? keyBackupSessionControllerProvider(key)
          : tokenSessionControllerProvider(key),
    ),
  );
});

/// No automatic retry: account pages show the failure with a Try again
/// action instead of silently re-sending requests.
Duration? _noRetry(int retryCount, Object error) => null;

/// The signed-in principal's global profile. Invalidate to reload.
final accountProfileProvider = FutureProvider.autoDispose
    .family<AccountProfile, SessionKey>(
      (ref, key) => ref.watch(accountApiProvider(key)).me(),
      retry: _noRetry,
    );

/// The account session of the active community: its token session, or the
/// session of its Google key backup. `null` when the community has no
/// account (an older key-only community), which hides account features.
final accountSessionProvider = Provider<SessionKey?>(
  (ref) =>
      ref.watch(activeTokenSessionProvider) ??
      ref.watch(activeGoogleKeySessionProvider),
);

/// This account's signed-in devices. Invalidate to reload.
final accountDevicesProvider = FutureProvider.autoDispose
    .family<List<AccountDevice>, SessionKey>(
      (ref, key) => ref.watch(accountApiProvider(key)).listDevices(),
      retry: _noRetry,
    );
