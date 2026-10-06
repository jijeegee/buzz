import 'package:flutter/foundation.dart';
import 'package:package_info_plus/package_info_plus.dart';

/// Production callback remains compatible with existing relay allowlists.
const buzzMobileCallbackScheme = 'xyz.block.buzz';
const buzzMobileRedirectUri = '$buzzMobileCallbackScheme://auth/cb';

/// Matches Android's debug manifest placeholder, including worktree overrides.
/// A side-by-side app must never receive the production app's OAuth callback.
String callbackSchemeForAndroidPackage(String packageName) {
  const productionPackage = 'xyz.block.buzz.mobile';
  if (packageName == productionPackage) return buzzMobileCallbackScheme;
  if (!packageName.startsWith('$productionPackage.')) {
    throw StateError('Unrecognized Android application identity.');
  }
  final suffix = packageName.substring(productionPackage.length);
  if (!RegExp(r'^\.[a-z][a-z0-9_]*$').hasMatch(suffix)) {
    throw StateError('Invalid Android application identity suffix.');
  }
  // Custom URI schemes cannot contain underscores; Gradle applies the same
  // conversion to its validated applicationIdSuffix.
  return '$buzzMobileCallbackScheme${suffix.replaceAll('_', '-')}';
}

/// Resolve from the installed Android package, not a separately supplied Dart
/// define that could diverge from the manifest. Apple callbacks stay unchanged.
Future<String> installedMobileCallbackScheme() async {
  if (defaultTargetPlatform != TargetPlatform.android) {
    return buzzMobileCallbackScheme;
  }
  return callbackSchemeForAndroidPackage(
    (await PackageInfo.fromPlatform()).packageName,
  );
}
