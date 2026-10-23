import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:open_filex/open_filex.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:path_provider/path_provider.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

import '../auth/default_community.dart';
import '../relay/relay_provider.dart';

/// Header the app sends on every relay WebSocket upgrade:
/// `<platform>/<version>`. The relay refuses an announced app below the
/// platform's minimum version with HTTP 426 (see the relay's `app_update`).
const appUpdateHeader = 'X-Kuzz-App';

/// Update channel name of this platform on the relay, or `null` where the
/// app does not self-update (iOS updates through the store).
String? get appUpdatePlatform => Platform.isAndroid ? 'android' : null;

String? _cachedVersion;

/// This build's visible version (`0.6.1`), read once.
Future<String> appVersion() async =>
    _cachedVersion ??= (await PackageInfo.fromPlatform()).version;

/// Value of [appUpdateHeader] for this build, or `null` when the platform
/// does not take part in updates.
Future<String?> appIdentityHeader() async {
  final platform = appUpdatePlatform;
  if (platform == null) return null;
  return '$platform/${await appVersion()}';
}

/// Raised by the relay socket when the relay refused this build (HTTP 426).
/// [AppUpdateNotifier] listens and turns it into the full-screen update.
final updateRequiredSignal = ValueNotifier<int>(0);

/// Whether a failed WebSocket upgrade was the relay's "update required"
/// refusal (HTTP 426). `IOWebSocketChannel.ready` wraps dart:io's error.
bool isUpdateRequiredRefusal(Object error) {
  final inner = error is WebSocketChannelException ? error.inner : error;
  return inner is WebSocketException && inner.httpStatusCode == 426;
}

/// The relay's answer for this platform (`GET /api/app-update/<platform>`).
@immutable
class AppRelease {
  const AppRelease({
    required this.version,
    required this.url,
    required this.updateAvailable,
    required this.required,
    this.notes,
  });

  factory AppRelease.fromJson(Map<String, dynamic> json) => AppRelease(
    version: json['version'] as String,
    url: json['url'] as String,
    notes: json['notes'] as String?,
    updateAvailable: json['update_available'] == true,
    required: json['required'] == true,
  );

  /// Version to install.
  final String version;

  /// APK download URL.
  final String url;

  /// Release notes, if the operator wrote any.
  final String? notes;

  /// This build is older than [version].
  final bool updateAvailable;

  /// This build is below the minimum version and must not be used.
  final bool required;
}

/// Where the update flow stands.
enum AppUpdatePhase { idle, downloading, installing, failed }

@immutable
class AppUpdateState {
  const AppUpdateState({
    this.release,
    this.forced = false,
    this.phase = AppUpdatePhase.idle,
    this.progress,
    this.dismissedVersion,
  });

  /// Latest answer that offers an update; `null` while there is none.
  final AppRelease? release;

  /// The relay refused this build or the answer says it is required.
  final bool forced;

  final AppUpdatePhase phase;

  /// Download progress 0..1, `null` when the size is unknown.
  final double? progress;

  /// Optional update the user postponed ("Later") in this run of the app.
  final String? dismissedVersion;

  /// Show the blocking screen.
  bool get blocking => forced;

  /// Offer the optional update.
  bool get offering =>
      !forced && release != null && release!.version != dismissedVersion;

  AppUpdateState copyWith({
    AppRelease? release,
    bool? forced,
    AppUpdatePhase? phase,
    double? progress,
    bool clearProgress = false,
    String? dismissedVersion,
  }) => AppUpdateState(
    release: release ?? this.release,
    forced: forced ?? this.forced,
    phase: phase ?? this.phase,
    progress: clearProgress ? null : progress ?? this.progress,
    dismissedVersion: dismissedVersion ?? this.dismissedVersion,
  );
}

/// Checks the relay for a newer build at start, on every return to the
/// foreground (at most every 30 minutes) and right away when the relay
/// refuses this build; downloads the APK and hands it to the system installer.
class AppUpdateNotifier extends Notifier<AppUpdateState> {
  static const _recheckAfter = Duration(minutes: 30);

  AppLifecycleListener? _lifecycle;
  DateTime? _lastCheck;
  bool _checking = false;

  @override
  AppUpdateState build() {
    if (appUpdatePlatform == null) return const AppUpdateState();
    _lifecycle = AppLifecycleListener(
      onResume: () {
        final last = _lastCheck;
        if (last == null || DateTime.now().difference(last) > _recheckAfter) {
          unawaited(check());
        }
      },
    );
    updateRequiredSignal.addListener(_onRefused);
    ref.onDispose(() {
      _lifecycle?.dispose();
      updateRequiredSignal.removeListener(_onRefused);
    });
    Future.microtask(check);
    return const AppUpdateState();
  }

  void _onRefused() {
    state = state.copyWith(forced: true);
    unawaited(check());
  }

  /// Asks the relay which build this app should run.
  Future<void> check() async {
    final platform = appUpdatePlatform;
    final origin = appBuiltInServerOrigin();
    if (platform == null || origin == null || origin == unsetServerOrigin) {
      return;
    }
    if (_checking) return;
    _checking = true;
    _lastCheck = DateTime.now();
    try {
      final current = await appVersion();
      final uri = Uri.parse(
        '$origin/api/app-update/$platform',
      ).replace(queryParameters: {'current': current});
      final pubkey = ref.read(myPubkeyProvider);
      final response = await http
          .get(
            uri,
            headers: {'Cache-Control': 'no-cache', 'X-Kuzz-Pubkey': ?pubkey},
          )
          .timeout(const Duration(seconds: 15));
      if (response.statusCode != 200) return;
      final release = AppRelease.fromJson(
        jsonDecode(response.body) as Map<String, dynamic>,
      );
      if (!release.updateAvailable) return;
      state = state.copyWith(
        release: release,
        forced: state.forced || release.required,
      );
    } catch (error) {
      debugPrint('[AppUpdate] check failed: $error');
    } finally {
      _checking = false;
    }
  }

  /// "Later" on the optional prompt; asked again on the next app start.
  void dismiss() {
    final release = state.release;
    if (release == null || state.forced) return;
    state = state.copyWith(dismissedVersion: release.version);
  }

  /// Downloads the APK and opens the system installer. Android replaces the
  /// app in place (same signing key) and its "Open" button restarts it.
  Future<void> install() async {
    final release = state.release;
    if (release == null || state.phase == AppUpdatePhase.downloading) {
      if (release == null) unawaited(check());
      return;
    }
    state = state.copyWith(
      phase: AppUpdatePhase.downloading,
      clearProgress: true,
    );
    final client = http.Client();
    try {
      final dir = await getTemporaryDirectory();
      final file = File('${dir.path}/kuzz-update-${release.version}.apk');
      final response = await client.send(
        http.Request('GET', Uri.parse(release.url)),
      );
      if (response.statusCode != 200) {
        throw HttpException('download failed (${response.statusCode})');
      }
      final total = response.contentLength;
      var received = 0;
      final sink = file.openWrite();
      try {
        await for (final chunk in response.stream) {
          sink.add(chunk);
          received += chunk.length;
          if (total != null && total > 0) {
            state = state.copyWith(progress: received / total);
          }
        }
      } finally {
        await sink.close();
      }
      state = state.copyWith(phase: AppUpdatePhase.installing);
      final result = await OpenFilex.open(
        file.path,
        type: 'application/vnd.android.package-archive',
      );
      if (result.type != ResultType.done) {
        throw StateError(result.message);
      }
      // The installer is in front now; if the user backs out, let them retry.
      state = state.copyWith(phase: AppUpdatePhase.idle, clearProgress: true);
    } catch (error) {
      debugPrint('[AppUpdate] install failed: $error');
      state = state.copyWith(phase: AppUpdatePhase.failed, clearProgress: true);
    } finally {
      client.close();
    }
  }
}

final appUpdateProvider = NotifierProvider<AppUpdateNotifier, AppUpdateState>(
  AppUpdateNotifier.new,
);
