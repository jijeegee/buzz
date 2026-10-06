import 'dart:async';
import 'dart:collection';
import 'dart:convert';

import 'package:buzz/shared/auth/token/token.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

/// In-memory [RefreshTokenStore] with injectable failures.
class FakeRefreshTokenStore implements RefreshTokenStore {
  final Map<String, StoredTokenSession> data = {};
  Object? writeError;
  Object? readError;
  Object? deleteError;
  int writes = 0;
  int deletes = 0;

  /// When set, writes wait for this completer (to interleave operations).
  Completer<void>? writeGate;

  @override
  Future<StoredTokenSession?> read(String origin) async {
    final error = readError;
    if (error != null) throw error;
    return data[origin];
  }

  @override
  Future<void> write(String origin, StoredTokenSession session) async {
    final gate = writeGate;
    if (gate != null) await gate.future;
    final error = writeError;
    if (error != null) throw error;
    writes += 1;
    data[origin] = session;
  }

  @override
  Future<void> delete(String origin) async {
    final error = deleteError;
    if (error != null) throw error;
    deletes += 1;
    data.remove(origin);
  }
}

/// [WebAuthLauncher] that answers with a scripted callback.
class FakeWebAuthLauncher implements WebAuthLauncher {
  FakeWebAuthLauncher(this.respond);

  String installedScheme = buzzMobileCallbackScheme;

  @override
  Future<String> callbackScheme() async => installedScheme;

  /// Builds the callback from the start URL the app opened.
  Uri Function(Uri startUrl) respond;
  final List<Uri> opened = [];
  final List<String> schemes = [];

  /// Throw instead of answering (e.g. [WebAuthCancelledException]).
  Object? error;

  /// When set, the browser stays open until this completes.
  Completer<void>? gate;

  @override
  Future<Uri> authenticate({
    required Uri url,
    required String callbackUrlScheme,
  }) async {
    opened.add(url);
    schemes.add(callbackUrlScheme);
    final pending = gate;
    if (pending != null) await pending.future;
    final failure = error;
    if (failure != null) throw failure;
    return respond(url);
  }

  /// A successful provider callback echoing the request `state`.
  static Uri success(Uri startUrl, {String code = 'bzl_code'}) =>
      Uri.parse(startUrl.queryParameters['redirect_uri']!).replace(
        queryParameters: {
          'code': code,
          'state': startUrl.queryParameters['state']!,
        },
      );
}

/// Scripted relay `/auth/*` endpoints for a [MockClient].
class FakeAuthServer {
  final Queue<http.Response Function(http.Request)> refreshResponses = Queue();
  final List<http.Request> requests = [];
  http.Response Function(http.Request) complete = (request) => jsonResponse({
    'principal_id': 'p' * 64,
    'device_id': 'device-1',
    'access': 'bzs_login',
    'refresh': 'bzr_login',
    'expires_in': 3600,
  });
  http.Response Function(http.Request) logout = (_) => http.Response('', 204);

  /// When set, `/auth/oidc/complete` answers with this future instead.
  Completer<http.Response>? heldComplete;

  /// Hold cleanup while a newer session transition finishes.
  Completer<http.Response>? heldLogout;

  /// NIP-11 document at `/`; defaults to a token relay offering Google.
  http.Response Function(http.Request) nip11 = (_) => jsonResponse({
    'name': 'Test relay',
    'buzz_token_auth': {
      'version': 1,
      'bearer': true,
      'oidc_providers': ['google'],
    },
  });

  /// Pending refresh responses, released manually to interleave operations.
  final Queue<Completer<http.Response>> heldRefreshes = Queue();
  bool holdRefreshes = false;

  int get refreshCalls =>
      requests.where((r) => r.url.path == '/auth/refresh').length;

  late final http.Client client = MockClient((request) async {
    requests.add(request);
    switch (request.url.path) {
      case '/':
        return nip11(request);
      case '/auth/oidc/complete':
        final held = heldComplete;
        if (held != null) return held.future;
        return complete(request);
      case '/auth/logout':
        final held = heldLogout;
        if (held != null) return held.future;
        return logout(request);
      case '/auth/refresh':
        if (holdRefreshes) {
          final held = Completer<http.Response>();
          heldRefreshes.add(held);
          return held.future;
        }
        if (refreshResponses.isEmpty) {
          throw StateError('unexpected refresh ${request.body}');
        }
        return refreshResponses.removeFirst()(request);
    }
    return http.Response('not found', 404);
  });

  /// A successful rotation response.
  static http.Response rotated(
    String access,
    String refresh, {
    int ttl = 3600,
  }) => jsonResponse({'access': access, 'refresh': refresh, 'expires_in': ttl});

  static http.Response authError(int status, String code) =>
      jsonResponse({'error': code, 'code': code}, status: status);
}

http.Response jsonResponse(Object body, {int status = 200}) => http.Response(
  jsonEncode(body),
  status,
  headers: {'content-type': 'application/json'},
);

/// Manually driven clock + timers for [TokenSessionController].
class FakeClock {
  FakeClock(this.now);

  DateTime now;
  final List<FakeTimer> timers = [];

  DateTime call() => now;

  Timer createTimer(Duration duration, void Function() callback) {
    final timer = FakeTimer(now.add(duration), callback, this);
    timers.add(timer);
    return timer;
  }

  List<FakeTimer> get active => timers.where((t) => t.isActive).toList();

  /// Advance time and fire every timer that came due.
  void advance(Duration duration) {
    now = now.add(duration);
    for (final timer in [...timers]) {
      if (timer.isActive && !timer.dueAt.isAfter(now)) timer.fire();
    }
  }
}

class FakeTimer implements Timer {
  FakeTimer(this.dueAt, this._callback, this._clock);

  final DateTime dueAt;
  final void Function() _callback;
  final FakeClock _clock;
  bool _active = true;

  Duration get delay => dueAt.difference(_clock.now);

  void fire() {
    _active = false;
    _callback();
  }

  @override
  void cancel() => _active = false;

  @override
  bool get isActive => _active;

  @override
  int get tick => 0;
}

/// Let pending microtasks and queued futures settle.
Future<void> settle() async {
  for (var i = 0; i < 20; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}
