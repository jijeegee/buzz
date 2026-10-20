import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import '../channels/channel.dart';

/// One group as the relay's groups routes describe it.
@immutable
class GroupSummary {
  const GroupSummary({
    required this.id,
    required this.name,
    required this.description,
    required this.channelType,
    required this.memberCount,
    required this.isMember,
    this.handle,
    this.visibility,
    this.invitePolicy,
    this.picture,
  });

  final String id;
  final String name;
  final String description;
  final String channelType;
  final int memberCount;
  final bool isMember;

  /// Public address without `@`, when the group has one.
  final String? handle;

  /// `open` or `private`. Search and by-handle results are always open.
  final String? visibility;

  /// `members` or `admins`, only from `GET /api/groups/{id}`.
  final String? invitePolicy;

  /// Group photo URL, or null when the group has none.
  final String? picture;

  bool get isPrivate => visibility == 'private';

  factory GroupSummary.fromJson(
    Map<String, dynamic> json, {
    String? defaultVisibility,
  }) {
    final handle = json['handle'];
    final picture = json['picture'];
    return GroupSummary(
      id: json['id'] as String,
      name: (json['name'] as String?) ?? '',
      description: (json['description'] as String?) ?? '',
      channelType: (json['channelType'] as String?) ?? 'stream',
      memberCount: (json['memberCount'] as num?)?.toInt() ?? 0,
      isMember: json['isMember'] as bool? ?? false,
      handle: handle is String && handle.isNotEmpty ? handle : null,
      visibility: (json['visibility'] as String?) ?? defaultVisibility,
      invitePolicy: json['invitePolicy'] as String?,
      picture: picture is String && picture.trim().isNotEmpty
          ? picture.trim()
          : null,
    );
  }

  /// A [Channel] for the group's avatar, header and read-only timeline
  /// before it is in the local channel list.
  Channel toChannel() => Channel(
    id: id,
    name: name,
    channelType: channelType,
    visibility: visibility ?? 'open',
    description: description,
    createdBy: '',
    createdAt: DateTime.fromMillisecondsSinceEpoch(0),
    memberCount: memberCount,
    isMember: isMember,
    picture: picture,
  );
}

/// A pending invitation to join a group.
@immutable
class GroupInvitation {
  const GroupInvitation({
    required this.id,
    required this.group,
    required this.inviter,
    required this.createdAt,
  });

  final String id;
  final GroupSummary group;

  /// Inviter's hex key.
  final String inviter;
  final DateTime? createdAt;

  factory GroupInvitation.fromJson(Map<String, dynamic> json) =>
      GroupInvitation(
        id: json['id'] as String,
        group: GroupSummary.fromJson(
          json['channel'] as Map<String, dynamic>,
          defaultVisibility: 'private',
        ),
        inviter: ((json['inviter'] as String?) ?? '').toLowerCase(),
        createdAt: DateTime.tryParse((json['createdAt'] as String?) ?? ''),
      );
}

/// `{code, url}` of a group's invite link.
@immutable
class GroupInviteLink {
  const GroupInviteLink({required this.code, required this.url});

  final String code;
  final String url;

  factory GroupInviteLink.fromJson(Map<String, dynamic> json) =>
      GroupInviteLink(code: json['code'] as String, url: json['url'] as String);
}

/// A refused groups request: the HTTP status and the relay's `error` text.
class GroupsApiException implements Exception {
  const GroupsApiException(this.statusCode, this.message);

  final int statusCode;
  final String message;

  bool get isNotFound => statusCode == 404;
  bool get isConflict => statusCode == 409;

  @override
  String toString() => message;
}

/// The relay's groups HTTP routes (search, addresses, invite links,
/// who-can-add-me and invitations), with the app's usual relay auth.
class GroupsApi {
  GroupsApi({
    required http.Client client,
    required RelayConfig config,
    required RelayAccessTokens? accessTokens,
  }) : _client = client,
       _config = config,
       _accessTokens = accessTokens;

  final http.Client _client;
  final RelayConfig _config;
  final RelayAccessTokens? _accessTokens;

  static const _timeout = Duration(seconds: 15);

  /// Open groups matching [query] (2+ characters), at most 20.
  Future<List<GroupSummary>> search(String query, {int limit = 20}) async {
    final body = await _send(
      'GET',
      '/api/groups/search',
      query: {'q': query.trim(), 'limit': '$limit'},
    );
    final groups = (body as Map<String, dynamic>)['groups'] as List<dynamic>;
    return [
      for (final item in groups)
        GroupSummary.fromJson(
          item as Map<String, dynamic>,
          defaultVisibility: 'open',
        ),
    ];
  }

  /// The open group with public address [handle] (no `@`).
  Future<GroupSummary> byHandle(String handle) async => GroupSummary.fromJson(
    await _send('GET', '/api/groups/by-handle/${Uri.encodeComponent(handle)}')
        as Map<String, dynamic>,
    defaultVisibility: 'open',
  );

  /// The group with its visibility and invite policy.
  Future<GroupSummary> group(String id) async => GroupSummary.fromJson(
    await _send('GET', '/api/groups/$id') as Map<String, dynamic>,
  );

  /// Sets or clears (null) the public address.
  Future<GroupSummary> setHandle(String id, String? handle) async =>
      GroupSummary.fromJson(
        await _send('PUT', '/api/groups/$id/handle', body: {'handle': handle})
            as Map<String, dynamic>,
        defaultVisibility: 'open',
      );

  Future<GroupInviteLink> inviteLink(String id) async =>
      GroupInviteLink.fromJson(
        await _send('GET', '/api/groups/$id/invite-link')
            as Map<String, dynamic>,
      );

  Future<GroupInviteLink> resetInviteLink(String id) async =>
      GroupInviteLink.fromJson(
        await _send('POST', '/api/groups/$id/invite-link/reset', body: {})
            as Map<String, dynamic>,
      );

  /// The group an invite code points to.
  Future<GroupSummary> previewInvite(String code) async =>
      GroupSummary.fromJson(
        await _send('GET', '/api/groups/invite/${Uri.encodeComponent(code)}')
            as Map<String, dynamic>,
      );

  /// Joins through an invite code; returns the group id.
  Future<String> joinInvite(String code) async {
    final body = await _send(
      'POST',
      '/api/groups/invite/${Uri.encodeComponent(code)}/join',
      body: {},
    );
    return (body as Map<String, dynamic>)['channelId'] as String;
  }

  Future<String> setInvitePolicy(String id, String policy) async {
    final body = await _send(
      'PUT',
      '/api/groups/$id/invite-policy',
      body: {'policy': policy},
    );
    return (body as Map<String, dynamic>)['policy'] as String;
  }

  /// `friends` or `everyone`.
  Future<String> addPolicy() async {
    final body = await _send('GET', '/api/me/group-add-policy');
    return (body as Map<String, dynamic>)['policy'] as String;
  }

  Future<String> setAddPolicy(String policy) async {
    final body = await _send(
      'PUT',
      '/api/me/group-add-policy',
      body: {'policy': policy},
    );
    return (body as Map<String, dynamic>)['policy'] as String;
  }

  Future<List<GroupInvitation>> invitations() async {
    final body = await _send('GET', '/api/group-invitations');
    return [
      for (final item in body as List<dynamic>)
        GroupInvitation.fromJson(item as Map<String, dynamic>),
    ];
  }

  /// Accepts an invitation; returns the group id.
  Future<String> acceptInvitation(String id) async {
    final body = await _send(
      'POST',
      '/api/group-invitations/$id/accept',
      body: {},
    );
    return (body as Map<String, dynamic>)['channelId'] as String;
  }

  Future<void> declineInvitation(String id) async {
    await _send('POST', '/api/group-invitations/$id/decline', body: {});
  }

  Future<Object?> _send(
    String method,
    String path, {
    Map<String, String>? query,
    Map<String, Object?>? body,
  }) async {
    final base = Uri.parse(_config.baseUrl).resolve(path);
    final uri = query == null ? base : base.replace(queryParameters: query);
    final url = uri.toString();
    final bodyBytes = body == null
        ? const <int>[]
        : utf8.encode(jsonEncode(body));
    Future<http.Response> send(String authorization) {
      final headers = {
        'Authorization': authorization,
        if (body != null) 'Content-Type': 'application/json',
      };
      final request = switch (method) {
        'GET' => _client.get(uri, headers: headers),
        'PUT' => _client.put(uri, headers: headers, body: bodyBytes),
        _ => _client.post(uri, headers: headers, body: bodyBytes),
      };
      return request.timeout(_timeout);
    }

    final response = await sendRelayAuthorized(
      _config,
      _accessTokens,
      send,
      nip98: () => buildNip98AuthHeader(
        method: method,
        url: url,
        bodyBytes: bodyBytes,
        nsec: _config.nsec,
      ),
    );
    Object? decoded;
    try {
      decoded = response.body.isEmpty ? null : jsonDecode(response.body);
    } on FormatException {
      decoded = null;
    }
    if (response.statusCode < 200 || response.statusCode >= 300) {
      final error = decoded is Map<String, dynamic> ? decoded['error'] : null;
      throw GroupsApiException(
        response.statusCode,
        error is String ? error : 'HTTP ${response.statusCode}',
      );
    }
    return decoded;
  }
}

/// Groups providers do not retry on their own: a refused request (no
/// access, gone) stays refused, and the screens refresh when opened.
Duration? noGroupsRetry(int retryCount, Object error) => null;

/// HTTP client for the groups routes.
final groupsHttpClientProvider = Provider<http.Client>((ref) {
  final client = http.Client();
  ref.onDispose(client.close);
  return client;
});

/// Groups routes bound to the active community.
final groupsApiProvider = Provider<GroupsApi>((ref) {
  final config = ref.watch(relayConfigProvider);
  return GroupsApi(
    client: ref.watch(groupsHttpClientProvider),
    config: config,
    accessTokens: config.tokenAuth
        ? ref.watch(relayAccessTokensProvider)
        : null,
  );
});
