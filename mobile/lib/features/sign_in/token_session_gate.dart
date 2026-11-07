import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/auth.dart';
import '../../shared/auth/token/token.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import 'community_recovery_actions.dart';
import 'token_sign_in_page.dart';

/// Shows [child] only while the token account [sessionKey] has a usable
/// session; otherwise the restore splash, the sign-in page, or a recovery
/// view that always offers "Retry", "Sign in again", switching to another
/// community and removing this one.
///
/// A stored session of the account the community records (`resuming`) shows
/// [child] at once while its first rotation runs behind it. A session that
/// is only reconnecting (retrying / stalled with a saved
/// principal) keeps [child] usable under a non-blocking banner, so cached
/// content and the app's own community switcher stay reachable
/// (AGENTS.md rule 6).
class TokenSessionGate extends ConsumerWidget {
  const TokenSessionGate({
    super.key,
    required this.sessionKey,
    required this.child,
  });

  final SessionKey sessionKey;
  final Widget child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final session = ref.watch(tokenSessionProvider(sessionKey));
    final active = ref.watch(activeCommunityProvider).value;
    final principal = session.principalId;
    final reconnecting =
        session.status == TokenSessionStatus.retrying ||
        session.status == TokenSessionStatus.stalled;
    // The child may only run under the identity the community records.
    final principalMatches =
        principal != null &&
        (active == null || !active.tokenAuth || active.pubkey == principal);
    // Drawing before the first rotation needs the exact account shown.
    final showsPrincipal =
        principal != null &&
        active != null &&
        active.tokenAuth &&
        active.pubkey == principal;

    return switch (session.status) {
      TokenSessionStatus.signedIn => _SessionShell(child: child),
      // The stored account draws from device storage while its token
      // rotates; no banner (AGENTS.md rule 6).
      TokenSessionStatus.resuming when showsPrincipal => _SessionShell(
        child: child,
      ),
      TokenSessionStatus.restoring || TokenSessionStatus.resuming =>
        _GateSplash(semanticLabel: context.l10n.signInSigningIn),
      TokenSessionStatus.signedOut ||
      TokenSessionStatus.signingIn => TokenSignInPage(
        lockedOrigin: sessionKey.origin,
        lockedSession: sessionKey,
      ),
      TokenSessionStatus.retrying || TokenSessionStatus.stalled
          when reconnecting && principalMatches =>
        _SessionShell(
          banner: _SessionRecoveryBanner(
            sessionKey: sessionKey,
            session: session,
          ),
          child: child,
        ),
      TokenSessionStatus.retrying || TokenSessionStatus.stalled =>
        _SessionRecoveryView(sessionKey: sessionKey, session: session),
    };
  }
}

/// Whether "Sign in again" is running for an account. Kept outside the
/// widgets: the session moving between the banner and the recovery view must
/// not allow a second attempt.
class _SignInAgainBusy extends Notifier<bool> {
  _SignInAgainBusy(this.sessionKey);

  final SessionKey sessionKey;

  @override
  bool build() => false;

  bool start() {
    if (state) return false;
    state = true;
    return true;
  }

  void finish() => state = false;
}

final _signInAgainBusyProvider =
    NotifierProvider.family<_SignInAgainBusy, bool, SessionKey>(
      _SignInAgainBusy.new,
    );

/// "Sign in again": a sign-in attempt that the account takes over. Returns
/// the failure to show, or `null`. Single-flight per account.
Future<String?> _signInAgain(
  BuildContext context,
  WidgetRef ref,
  SessionKey sessionKey,
) async {
  final busy = ref.read(_signInAgainBusyProvider(sessionKey).notifier);
  if (!busy.start()) return null;
  final failed = context.l10n.signInFinishFailed;
  final auth = ref.read(authProvider.notifier);
  final attempt = ref.read(tokenSignInAttemptProvider)(sessionKey.origin);
  try {
    if (!await attempt.signIn()) return attempt.state.errorMessage;
    await auth.authenticateWithTokenSession(
      relayUrl: sessionKey.origin,
      principalId: attempt.state.principalId!,
      attempt: attempt,
    );
    return null;
  } catch (error) {
    debugPrint('Could not sign in again: ${error.runtimeType}');
    return failed;
  } finally {
    busy.finish();
    // Handed over, this ends nothing: the attempt holds no tokens anymore.
    unawaited(attempt.discard().whenComplete(attempt.dispose));
  }
}

class _GateSplash extends StatelessWidget {
  const _GateSplash({required this.semanticLabel});

  final String semanticLabel;

  @override
  Widget build(BuildContext context) => Scaffold(
    body: Center(
      child: KuzzLoadingIndicator(size: 56, semanticLabel: semanticLabel),
    ),
  );
}

/// Keeps [child] at a stable position so showing or hiding the banner does
/// not remount the app below it.
class _SessionShell extends StatelessWidget {
  const _SessionShell({required this.child, this.banner});

  final Widget child;
  final Widget? banner;

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        if (banner != null)
          KeyedSubtree(key: const Key('banner'), child: banner!),
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

class _SessionRecoveryBanner extends ConsumerWidget {
  const _SessionRecoveryBanner({
    required this.sessionKey,
    required this.session,
  });

  final SessionKey sessionKey;
  final TokenSessionState session;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final controller = ref
        .read(tokenSessionProvider(sessionKey).notifier)
        .controller;
    final retrying = session.status == TokenSessionStatus.retrying;
    final busy = ref.watch(_signInAgainBusyProvider(sessionKey));
    return Material(
      key: const Key('token-session-banner'),
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
                child: Semantics(
                  liveRegion: true,
                  child: Text(
                    retrying
                        ? context.l10n.signInReconnectingBanner
                        : context.l10n.signInUnreachableBanner,
                    style: context.textTheme.bodySmall?.copyWith(
                      color: context.colors.onSurfaceVariant,
                    ),
                  ),
                ),
              ),
              if (busy)
                KuzzLoadingIndicator(
                  key: const Key('token-session-sign-in-again-busy'),
                  size: Grid.sm,
                  semanticLabel: context.l10n.signInSigningIn,
                )
              else ...[
                TextButton(
                  key: const Key('token-session-retry'),
                  onPressed: () => unawaited(controller.retryNow()),
                  child: Text(context.l10n.signInRetryNow),
                ),
                TextButton(
                  key: const Key('token-session-sign-in-again'),
                  onPressed: () =>
                      unawaited(_signInAgain(context, ref, sessionKey)),
                  child: Text(context.l10n.signInSignInAgain),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

class _SessionRecoveryView extends HookConsumerWidget {
  const _SessionRecoveryView({required this.sessionKey, required this.session});

  final SessionKey sessionKey;
  final TokenSessionState session;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final controller = ref
        .read(tokenSessionProvider(sessionKey).notifier)
        .controller;
    final retrying = session.status == TokenSessionStatus.retrying;
    final signInError = useState<String?>(null);
    final busy = ref.watch(_signInAgainBusyProvider(sessionKey));

    Future<void> signInAgain() async {
      final error = await _signInAgain(context, ref, sessionKey);
      if (context.mounted) signInError.value = error;
    }

    return Scaffold(
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(Grid.sm),
          children: [
            const SizedBox(height: Grid.lg),
            Text(
              retrying
                  ? context.l10n.signInReconnecting
                  : context.l10n.signInUnreachableTitle,
              style: context.textTheme.headlineSmall,
            ),
            const SizedBox(height: Grid.xxs),
            Semantics(
              liveRegion: true,
              child: Text(
                signInError.value ??
                    session.errorMessage ??
                    context.l10n.signInUnreachableBody,
                style: context.textTheme.bodyMedium?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
            ),
            const SizedBox(height: Grid.sm),
            FilledButton(
              key: const Key('token-session-retry'),
              onPressed: busy ? null : () => unawaited(controller.retryNow()),
              child: Text(context.l10n.signInRetryNow),
            ),
            const SizedBox(height: Grid.xxs),
            if (busy)
              Center(
                child: KuzzLoadingIndicator(
                  key: const Key('token-session-sign-in-again-busy'),
                  size: Grid.md,
                  semanticLabel: context.l10n.signInSigningIn,
                ),
              )
            else
              TextButton(
                key: const Key('token-session-sign-in-again'),
                onPressed: () => unawaited(signInAgain()),
                child: Text(context.l10n.signInSignInAgain),
              ),
            const SizedBox(height: Grid.md),
            const CommunityRecoveryActions(),
          ],
        ),
      ),
    );
  }
}
