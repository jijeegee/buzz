import 'dart:async';

import 'package:sentry_flutter/sentry_flutter.dart';

import '../auth/default_community.dart';
import 'error_report_scrub.dart';

/// Crash reporting address, `--dart-define=KUZZ_SENTRY_DSN_MOBILE=...`.
/// Unset builds send nothing.
const _dsn = String.fromEnvironment('KUZZ_SENTRY_DSN_MOBILE');

/// Runs [appRunner] with crash and error reporting when this build has a DSN.
///
/// Reports carry the error, where it happened, app version and OS only: no
/// user, IP, device name, screenshots, view hierarchy (off by default),
/// breadcrumbs (logs can hold message text), sessions or performance data,
/// and every Dart event goes through [scrubReportEvent]. The same error is
/// sent once per run.
Future<void> runWithErrorReporting(FutureOr<void> Function() appRunner) async {
  if (_dsn.trim().isEmpty) {
    await appRunner();
    return;
  }
  final admit = createReportGate();
  await SentryFlutter.init((options) {
    options
      ..dsn = _dsn.trim()
      ..environment = appKuzzEnvironment().name
      ..sendDefaultPii = false
      ..maxBreadcrumbs = 0
      ..enablePrintBreadcrumbs = false
      ..enableAutoNativeBreadcrumbs = false
      ..enableUserInteractionBreadcrumbs = false
      ..enableUserInteractionTracing = false
      ..enableAutoSessionTracking = false
      ..enableAutoPerformanceTracing = false
      ..enableFramesTracking = false
      ..enableMetrics = false
      ..attachScreenshot = false
      ..reportPackages = false
      // iOS reports failed URLSession requests natively, with the URL and
      // query, past beforeSend.
      ..captureFailedRequests = false
      ..captureNativeFailedRequests = false
      ..beforeSend = (event, hint) {
        final scrubbed = scrubReportEvent(event);
        return admit(scrubbed) ? scrubbed : null;
      };
  }, appRunner: appRunner);
}
