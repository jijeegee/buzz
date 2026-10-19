import 'package:flutter/foundation.dart';

/// What a group link points to: an invite code or a public address.
@immutable
sealed class GroupLinkTarget {
  const GroupLinkTarget();
}

/// An invite link code (`/g/<code>`, `kuzz://join/<code>` or a bare code).
class GroupInviteCodeTarget extends GroupLinkTarget {
  const GroupInviteCodeTarget(this.code);

  final String code;

  @override
  bool operator ==(Object other) =>
      other is GroupInviteCodeTarget && other.code == code;

  @override
  int get hashCode => code.hashCode;

  @override
  String toString() => 'GroupInviteCodeTarget($code)';
}

/// A public address (`/g/@<handle>`, `kuzz://join/@<handle>` or `@handle`),
/// lowercase and without `@`.
class GroupHandleTarget extends GroupLinkTarget {
  const GroupHandleTarget(this.handle);

  final String handle;

  @override
  bool operator ==(Object other) =>
      other is GroupHandleTarget && other.handle == handle;

  @override
  int get hashCode => handle.hashCode;

  @override
  String toString() => 'GroupHandleTarget($handle)';
}

final _codePattern = RegExp(r'^[A-Za-z0-9_-]{22,64}$');
final _handlePattern = RegExp(r'^[a-z0-9_]{3,32}$');

/// App schemes that carry `join/...` links: `kuzz` and side-by-side demo
/// builds (`<app>-demo-<slug>`), the same rule as desktop.
final _appSchemePattern = RegExp(r'^(kuzz|[a-z][a-z0-9]*-demo-[a-z0-9-]+)$');

/// Whether [code] is shaped like an invite code the relay mints.
bool isPlausibleInviteCode(String code) => _codePattern.hasMatch(code);

GroupLinkTarget? _fromSegment(String segment) {
  if (segment.startsWith('@')) {
    final handle = segment.substring(1).toLowerCase();
    return _handlePattern.hasMatch(handle) ? GroupHandleTarget(handle) : null;
  }
  return isPlausibleInviteCode(segment) ? GroupInviteCodeTarget(segment) : null;
}

/// Parses what someone pasted into Join with link:
/// `https://<host>/g/<code>`, `https://<host>/g/@<handle>`,
/// `kuzz://join/<code>`, `kuzz://join/@<handle>`, a bare code or `@handle`.
GroupLinkTarget? parseGroupLinkInput(String input) {
  final text = input.trim();
  if (text.isEmpty) return null;
  if (!text.contains('://')) {
    // A host without a scheme, as people often copy it.
    final slash = text.indexOf('/g/');
    if (slash > 0 && !text.substring(0, slash).contains('/')) {
      return parseGroupLinkInput('https://$text');
    }
    return text.contains('/') ? null : _fromSegment(text);
  }
  final uri = Uri.tryParse(text);
  if (uri == null) return null;
  return parseGroupLinkUri(uri);
}

/// Parses an `https://<host>/g/...` page link or an app `join/...` link.
GroupLinkTarget? parseGroupLinkUri(Uri uri) {
  if (uri.userInfo.isNotEmpty || uri.hasFragment) return null;
  final segments = [
    for (final segment in uri.pathSegments)
      if (segment.isNotEmpty) segment,
  ];
  if (uri.scheme == 'https' || uri.scheme == 'http') {
    if (uri.host.isEmpty || segments.length != 2 || segments[0] != 'g') {
      return null;
    }
    return _fromSegment(segments[1]);
  }
  return parseGroupJoinDeepLink(uri);
}

/// Parses `kuzz://join/<code>` and `kuzz://join/@<handle>`. The older
/// `kuzz://join?relay=…&code=…` community invite is not a group link.
GroupLinkTarget? parseGroupJoinDeepLink(Uri uri) {
  if (!_appSchemePattern.hasMatch(uri.scheme) || uri.host != 'join') {
    return null;
  }
  if (uri.hasQuery || uri.hasFragment || uri.userInfo.isNotEmpty) return null;
  final segments = [
    for (final segment in uri.pathSegments)
      if (segment.isNotEmpty) segment,
  ];
  if (segments.length != 1) return null;
  return _fromSegment(segments.single);
}
