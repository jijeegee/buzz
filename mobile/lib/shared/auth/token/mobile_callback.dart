import 'package:flutter/foundation.dart';
import 'package:package_info_plus/package_info_plus.dart';

/// Production callback remains compatible with existing relay allowlists.
const kuzzMobileCallbackScheme = 'app.kuzz';
const kuzzMobileRedirectUri = '$kuzzMobileCallbackScheme://auth/cb';

/// Matches Android's debug manifest placeholder, including worktree overrides.
/// A side-by-side app must never receive the production app's OAuth callback.
String callbackSchemeForAndroidPackage(String packageName) {
  const productionPackage = 'app.kuzz.mobile';
  if (packageName == productionPackage) return kuzzMobileCallbackScheme;
  if (!packageName.startsWith('$productionPackage.')) {
    throw StateError('Unrecognized Android application identity.');
  }
  final suffix = packageName.substring(productionPackage.length);
  if (!RegExp(r'^\.[a-z][a-z0-9_]*$').hasMatch(suffix)) {
    throw StateError('Invalid Android application identity suffix.');
  }
  // Custom URI schemes cannot contain underscores; Gradle applies the same
  // conversion to its validated applicationIdSuffix.
  return '$kuzzMobileCallbackScheme${suffix.replaceAll('_', '-')}';
}

/// Resolve from the installed Android package, not a separately supplied Dart
/// define that could diverge from the manifest. Apple callbacks stay unchanged.
Future<String> installedMobileCallbackScheme() async {
  if (defaultTargetPlatform != TargetPlatform.android) {
    return kuzzMobileCallbackScheme;
  }
  return callbackSchemeForAndroidPackage(
    (await PackageInfo.fromPlatform()).packageName,
  );
}
