import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

import '../token/token_auth_test_fakes.dart';

/// A relay with the `/auth/*` account endpoints on top of [FakeAuthServer]
/// (NIP-11, login, refresh, logout).
class FakeAccountServer {
  FakeAccountServer({FakeAuthServer? auth}) : auth = auth ?? FakeAuthServer();

  final FakeAuthServer auth;

  /// Every account request, in order (not the delegated auth ones).
  final List<http.Request> accountRequests = [];

  Map<String, Object?> profile = {
    'principal_id': 'p' * 64,
    'kind': 'user',
    'display_name': 'Ada',
    'avatar_url': null,
    'username': 'ada',
  };

  List<Map<String, Object?>> devices = [
    {
      'id': 'device-1',
      'name': 'Test phone',
      'platform': 'mobile',
      'last_seen_at': '2026-10-05T12:00:00Z',
      'current': true,
    },
    {
      'id': 'device-2',
      'name': 'Work laptop',
      'platform': 'desktop',
      'last_seen_at': '2026-10-04T09:30:00Z',
      'current': false,
    },
    {
      'id': 'device-3',
      'name': 'Old browser',
      'platform': 'web',
      'last_seen_at': '2026-09-01T09:30:00Z',
      'current': false,
    },
  ];

  /// When set, the next account request with this path answers with it.
  final Map<String, http.Response Function(http.Request)> failures = {};

  /// Requests for [path] (method + path, e.g. `PATCH /auth/profile`).
  List<http.Request> calls(String method, String path) => [
    for (final request in accountRequests)
      if (request.method == method && request.url.path == path) request,
  ];

  late final http.Client client = MockClient((request) async {
    final path = request.url.path;
    final isAccount =
        path == '/auth/me' ||
        path == '/auth/profile' ||
        path.startsWith('/auth/devices') ||
        path == '/auth/sessions/revoke-others' ||
        path == '/auth/bots/revoke-all' ||
        path == '/auth/account';
    if (!isAccount) {
      final copy = http.Request(request.method, request.url)
        ..headers.addAll(request.headers)
        ..bodyBytes = request.bodyBytes;
      return auth.client.send(copy).then(_drain);
    }
    accountRequests.add(request);
    final failure = failures.remove(path);
    if (failure != null) return failure(request);
    if (request.headers['Authorization'] == null) {
      return jsonResponse({
        'error': 'missing token',
        'code': 'unauthorized',
      }, status: 401);
    }
    switch ((request.method, path)) {
      case ('GET', '/auth/me'):
        return jsonResponse(profile);
      case ('PATCH', '/auth/profile'):
        final body = jsonDecode(request.body) as Map<String, Object?>;
        profile = {...profile, ...body};
        return jsonResponse({
          'principal_id': profile['principal_id'],
          'display_name': profile['display_name'],
          'avatar_url': profile['avatar_url'],
          'username': profile['username'],
        });
      case ('GET', '/auth/devices'):
        return jsonResponse(devices);
      case ('POST', '/auth/sessions/revoke-others'):
        devices = [
          for (final device in devices)
            if (device['current'] == true) device,
        ];
        return http.Response('', 204);
      case ('POST', '/auth/bots/revoke-all'):
      case ('DELETE', '/auth/account'):
        return http.Response('', 204);
    }
    if (request.method == 'PATCH' && path.startsWith('/auth/devices/')) {
      final id = path.substring('/auth/devices/'.length);
      final name = (jsonDecode(request.body) as Map<String, Object?>)['name'];
      final index = devices.indexWhere((device) => device['id'] == id);
      if (index < 0) return jsonResponse({'error': 'not found'}, status: 404);
      devices = [...devices]..[index] = {...devices[index], 'name': name};
      return jsonResponse(devices[index]);
    }
    if (request.method == 'DELETE' && path.startsWith('/auth/devices/')) {
      final id = path.substring('/auth/devices/'.length);
      final before = devices.length;
      devices = [
        for (final device in devices)
          if (device['id'] != id) device,
      ];
      if (devices.length == before) {
        return jsonResponse({'error': 'not found'}, status: 404);
      }
      return http.Response('', 204);
    }
    return http.Response('not found', 404);
  });

  static Future<http.Response> _drain(http.StreamedResponse streamed) =>
      http.Response.fromStream(streamed);
}
