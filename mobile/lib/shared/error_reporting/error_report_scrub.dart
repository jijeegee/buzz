import 'package:characters/characters.dart';
import 'package:sentry_flutter/sentry_flutter.dart';

/// Privacy filter for crash and error reports (see `error_reporting.dart`).
///
/// Mirrors desktop (`errorReportScrub.ts`, `error_reporting.rs`): reports keep
/// the error, where it happened, app version and OS only.

/// Most reports one run may send; a crash loop must not use up the quota.
const maxReportsPerRun = 20;

/// Longest error text kept, after scrubbing.
const maxReportTextChars = 300;

/// Longest input scrubbed; longer error text is cut first so a huge error
/// cannot stall the app in the regexes.
const _maxInputChars = 2000;

final _quoted = RegExp('`[^`]*`|"[^"]*"|\'[^\']{2,}\'|“[^”]*”|‘[^’]{2,}’');
final _userDir = RegExp(
  r'(\b[a-z]:[\\/]+users[\\/]+|/home/|/users/)[^\\/\r\n]+',
  caseSensitive: false,
);
final _urlQuery = RegExp(
  r'\b((?:https?|wss?)://[^\s?#]+)[?#]\S*',
  caseSensitive: false,
);
final _secrets = RegExp(
  [
    r'\b(?:nsec|npub|nprofile|nevent|naddr|note)1[0-9a-z]{10,}',
    r'[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}',
    r'\b[0-9a-f]{32,}\b',
    r'\bauthorization\s*[:=]\s*\S+(?:\s+\S+)?',
    r'\b(?:bearer|nostr|basic)\s+[A-Za-z0-9._~+/=\-]{8,}',
    r'\b(?:token|key|secret|password|auth)\s*[:=]\s*\S+',
    r'\b\d{1,3}(?:\.\d{1,3}){3}\b',
    r'\b[0-9a-f]*\d[0-9a-f]*(?::[0-9a-f]{0,4}){2,7}',
    r'[A-Za-z0-9+/_\-]{16,}={1,2}',
    r'[A-Za-z0-9_\-+]{40,}',
  ].join('|'),
  caseSensitive: false,
);

/// Scrubs one error string: quoted text (which can be message text), user
/// folder names, keys, tokens, emails, IP addresses and URL queries are
/// replaced, then cut to [maxReportTextChars].
String scrubReportText(String text) {
  final input = text.characters.take(_maxInputChars).toString();
  final scrubbed = input
      .replaceAll(_quoted, '[quoted]')
      .replaceAllMapped(_userDir, (m) => '${m[1]}[user]')
      .replaceAllMapped(_urlQuery, (m) => m[1]!)
      .replaceAll(_secrets, '[redacted]');
  final chars = scrubbed.characters;
  return chars.length <= maxReportTextChars
      ? scrubbed
      : '${chars.take(maxReportTextChars)}…';
}

String _fileName(String path) => path.split(RegExp(r'[\\/]')).last;

/// Removes everything a report must not carry.
SentryEvent scrubReportEvent(SentryEvent event) {
  event
    ..user = null
    ..request = null
    ..serverName = null
    ..breadcrumbs = null
    // Still sent when set, so clear it.
    // ignore: deprecated_member_use
    ..extra = null
    ..tags = null;
  event.contexts.device?.name = null;
  final message = event.message;
  if (message != null) {
    event.message = SentryMessage(scrubReportText(message.formatted));
  }
  for (final exception in event.exceptions ?? const <SentryException>[]) {
    final value = exception.value;
    if (value != null) exception.value = scrubReportText(value);
  }
  for (final image in event.debugMeta?.images ?? const <DebugImage>[]) {
    final codeFile = image.codeFile;
    if (codeFile != null) image.codeFile = _fileName(codeFile);
    final debugFile = image.debugFile;
    if (debugFile != null) image.debugFile = _fileName(debugFile);
  }
  return event;
}

String _reportKey(SentryEvent event) {
  final parts = <String>[event.message?.formatted ?? ''];
  for (final exception in event.exceptions ?? const <SentryException>[]) {
    final frames = exception.stackTrace?.frames;
    final top = frames == null || frames.isEmpty ? null : frames.last;
    parts.addAll([
      exception.type ?? '',
      exception.value ?? '',
      top?.function ?? '',
      '${top?.lineNo ?? ''}',
    ]);
  }
  return parts.join('\u0000');
}

/// Admits each distinct error once per run, up to [maxReportsPerRun].
/// Call with already scrubbed events.
bool Function(SentryEvent) createReportGate() {
  final seen = <String>{};
  return (event) {
    if (seen.length >= maxReportsPerRun) return false;
    return seen.add(_reportKey(event));
  };
}
