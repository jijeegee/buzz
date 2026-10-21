import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../auth/account/account_api.dart';
import '../auth/default_community.dart';
import 'relay_provider.dart';

export '../auth/default_community.dart'
    show regionRequestUrl, regionSocketUrl, serverGivenConnectUrl;

/// Where a request to [url] on the service is sent for [config]'s account:
/// its home region when the server gave one (token accounts only, as for
/// `/query`). Auth stays signed for the service address.
String regionUrlFor(RelayConfig config, String url) => config.tokenAuth
    ? regionRequestUrl(config.principalId, url, config.baseUrl)
    : url;

/// [regionUrlFor] for a WebSocket address.
String regionSocketUrlFor(RelayConfig config, String url) => config.tokenAuth
    ? regionSocketUrl(config.principalId, url, config.baseUrl)
    : url;

/// Where the active account's relay WebSocket connects: the `connect_url`
/// `/auth/me` returns (its home region) when [acceptConnectUrl] takes it,
/// else `null` (the service address). Remembered for the relay socket's
/// guard; failures keep the service address.
final serverGivenConnectUrlProvider = FutureProvider<String?>((ref) async {
  final origin = ref.watch(accountOriginProvider);
  if (origin == null) return null;
  final builtIn = ref.watch(defaultCommunityOriginProvider);
  final AccountProfile profile;
  try {
    profile = await ref.watch(accountApiProvider(origin)).me();
  } on Object {
    return null;
  }
  final accepted = acceptConnectUrl(profile.connectUrl, builtIn);
  rememberConnectUrl(profile.principalId, accepted);
  return accepted;
}, retry: (_, _) => null);
