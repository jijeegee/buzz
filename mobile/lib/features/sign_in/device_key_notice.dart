import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/auth/auth.dart';
import '../../shared/auth/token/token.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';

/// Relay code for a person session that signed in before device keys.
const deviceKeyRequiredCode = 'device_key_required';

/// Why the sign-in screen is shown, when signing out lost the session that
/// said so. Cleared when a sign-in starts.
final signInNoticeProvider = NotifierProvider<SignInNotice, String?>(
  SignInNotice.new,
);

class SignInNotice extends Notifier<String?> {
  @override
  String? build() => null;

  void show(String message) => state = message;

  void clear() => state = null;
}

/// Wraps the signed-in home. A session that signed in before device keys
/// (`/auth/me` `device_key: false`) gets a banner offering to sign in again.
/// When the relay already refuses a Google-key session with
/// `device_key_required`, that account is signed out here and the sign-in
/// screen says why (a token community's own gate shows the sign-in screen
/// with the session's message).
class DeviceKeyNotice extends HookConsumerWidget {
  const DeviceKeyNotice({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final origin = ref.watch(accountOriginProvider);
    final googleKey =
        origin != null && ref.watch(activeGoogleKeyOriginProvider) == origin;
    final session = googleKey
        ? ref.watch(keyBackupSessionProvider(origin))
        : null;
    final ended =
        session?.status == TokenSessionStatus.signedOut &&
        session?.endedCode == deviceKeyRequiredCode;
    final busy = useState(false);

    Future<void> signOut({required bool ended}) async {
      final active = ref.read(activeCommunityProvider).value;
      if (active == null || busy.value) return;
      busy.value = true;
      if (ended) {
        ref
            .read(signInNoticeProvider.notifier)
            .show(appL10n.tokenSessionDeviceKeyRequired);
      }
      final messenger = ScaffoldMessenger.maybeOf(context);
      try {
        await ref
            .read(authProvider.notifier)
            .signOutCommunity(active.id, deviceOnly: ended);
      } catch (_) {
        messenger?.showSnackBar(
          SnackBar(content: Text(appL10n.settingsSignOutFailed)),
        );
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    useEffect(() {
      if (ended) unawaited(signOut(ended: true));
      return null;
    }, [ended]);

    final profile = origin == null
        ? null
        : ref.watch(accountProfileProvider(origin)).value;
    final banner = profile?.deviceKey == false && !ended
        ? _DeviceKeyBanner(
            onSignIn: busy.value
                ? null
                : () => unawaited(signOut(ended: false)),
          )
        : null;
    // Same shell as the session recovery banner: the child keeps its place.
    return Column(
      children: [
        if (banner != null)
          KeyedSubtree(key: const Key('device-key-banner'), child: banner),
        Expanded(
          key: const Key('content'),
          child: MediaQuery.removePadding(
            context: context,
            removeTop: banner != null,
            child: child,
          ),
        ),
      ],
    );
  }
}

class _DeviceKeyBanner extends StatelessWidget {
  const _DeviceKeyBanner({required this.onSignIn});

  final VoidCallback? onSignIn;

  @override
  Widget build(BuildContext context) {
    return Material(
      color: context.colors.surfaceContainerHigh,
      child: SafeArea(
        bottom: false,
        child: Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: Grid.xs,
            vertical: Grid.xxs,
          ),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  context.l10n.signInDeviceKeyBanner,
                  style: context.textTheme.bodySmall?.copyWith(
                    color: context.colors.onSurfaceVariant,
                  ),
                ),
              ),
              TextButton(
                key: const Key('device-key-sign-in-again'),
                onPressed: onSignIn,
                child: Text(context.l10n.signInSignInAgain),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
