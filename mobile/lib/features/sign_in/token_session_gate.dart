import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/auth.dart';
import '../../shared/auth/token/token.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/buzz_loading_indicator.dart';
import 'community_recovery_actions.dart';
import 'token_sign_in_page.dart';

/// Shows [child] only while the token community at [origin] has a usable
/// session; otherwise the restore splash, the sign-in page, or a recovery
/// view that always offers "Retry", "Sign in again", switching to another
/// community and removing this one.
///
/// A session that is only reconnecting (retrying / stalled with a saved
/// principal) keeps [child] usable under a non-blocking banner, so cached
/// content and the app's own community switcher stay reachable
/// (AGENTS.md rule 6).
///
/// A re-sign-in may be a different account. The community records the
/// identity it shows, so [child] waits until that record matches the
/// session's principal; a failed write is shown with a retry instead of
/// letting the app run under the old identity (AGENTS.md rule 1).
class TokenSessionGate extends HookConsumerWidget {
  const TokenSessionGate({
    super.key,
    required this.origin,
    required this.child,
  });

  final String origin;
  final Widget child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final session = ref.watch(tokenSessionProvider(origin));
    final active = ref.watch(activeCommunityProvider).value;
    final principal = session.principalId;
    final staleIdentity =
        session.status == TokenSessionStatus.signedIn &&
        principal != null &&
        active != null &&
        active.tokenAuth &&
        active.pubkey != principal;
    final attempt = useState(0);
    final failure = useState<Object?>(null);
    useEffect(() {
      if (!staleIdentity) return null;
      var current = true;
      ref
          .read(authProvider.notifier)
          .authenticateWithTokenSession(
            relayUrl: origin,
            principalId: principal,
          )
          .then<void>(
            (_) {},
            onError: (Object error) {
              debugPrint('Could not record the new token identity: $error');
              if (current && context.mounted) failure.value = error;
            },
          );
      return () => current = false;
    }, [staleIdentity, principal, active?.pubkey, attempt.value]);

    final reconnecting =
        session.status == TokenSessionStatus.retrying ||
        session.status == TokenSessionStatus.stalled;
    // The child may only run under the identity the community records.
    final principalMatches =
        principal != null &&
        (active == null || !active.tokenAuth || active.pubkey == principal);

    return switch (session.status) {
      TokenSessionStatus.signedIn when staleIdentity =>
        failure.value == null
            ? const _GateSplash(semanticLabel: 'Updating your account')
            : _IdentityFailureView(
                error: failure.value!,
                onRetry: () {
                  failure.value = null;
                  attempt.value += 1;
                },
              ),
      TokenSessionStatus.signedIn => _SessionShell(child: child),
      TokenSessionStatus.restoring => const _GateSplash(
        semanticLabel: 'Signing in',
      ),
      TokenSessionStatus.signedOut ||
      TokenSessionStatus.signingIn => TokenSignInPage(lockedOrigin: origin),
      TokenSessionStatus.retrying || TokenSessionStatus.stalled
          when reconnecting && principalMatches =>
        _SessionShell(
          banner: _SessionRecoveryBanner(origin: origin, session: session),
          child: child,
        ),
      TokenSessionStatus.retrying || TokenSessionStatus.stalled =>
        _SessionRecoveryView(origin: origin, session: session),
    };
  }
}

class _GateSplash extends StatelessWidget {
  const _GateSplash({required this.semanticLabel});

  final String semanticLabel;

  @override
  Widget build(BuildContext context) => Scaffold(
    body: Center(
      child: BuzzLoadingIndicator(size: 56, semanticLabel: semanticLabel),
    ),
  );
}

class _IdentityFailureView extends StatelessWidget {
  const _IdentityFailureView({required this.error, required this.onRetry});

  final Object error;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(Grid.sm),
          children: [
            const SizedBox(height: Grid.lg),
            Text(
              'Couldn’t switch accounts',
              style: context.textTheme.headlineSmall,
            ),
            const SizedBox(height: Grid.xxs),
            Semantics(
              liveRegion: true,
              child: Text(
                'You signed in with a different account, but this device '
                'could not save it: $error',
                style: context.textTheme.bodyMedium?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
            ),
            const SizedBox(height: Grid.sm),
            FilledButton(
              key: const Key('token-identity-retry'),
              onPressed: onRetry,
              child: const Text('Try again'),
            ),
          ],
        ),
      ),
    );
  }
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
  const _SessionRecoveryBanner({required this.origin, required this.session});

  final String origin;
  final TokenSessionState session;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final controller = ref
        .read(tokenSessionProvider(origin).notifier)
        .controller;
    final retrying = session.status == TokenSessionStatus.retrying;
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
                        ? 'Reconnecting to your relay…'
                        : 'Can’t reach your relay. Your sign-in is saved.',
                    style: context.textTheme.bodySmall?.copyWith(
                      color: context.colors.onSurfaceVariant,
                    ),
                  ),
                ),
              ),
              TextButton(
                key: const Key('token-session-retry'),
                onPressed: () => unawaited(controller.retryNow()),
                child: const Text('Retry now'),
              ),
              TextButton(
                key: const Key('token-session-sign-in-again'),
                onPressed: () => unawaited(controller.signIn()),
                child: const Text('Sign in again'),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _SessionRecoveryView extends ConsumerWidget {
  const _SessionRecoveryView({required this.origin, required this.session});

  final String origin;
  final TokenSessionState session;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final controller = ref
        .read(tokenSessionProvider(origin).notifier)
        .controller;
    final retrying = session.status == TokenSessionStatus.retrying;
    return Scaffold(
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(Grid.sm),
          children: [
            const SizedBox(height: Grid.lg),
            Text(
              retrying ? 'Reconnecting…' : 'Can’t reach your relay',
              style: context.textTheme.headlineSmall,
            ),
            const SizedBox(height: Grid.xxs),
            Semantics(
              liveRegion: true,
              child: Text(
                session.errorMessage ??
                    'Your sign-in is saved. Buzz will keep it until you '
                        'retry or sign in again.',
                style: context.textTheme.bodyMedium?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
            ),
            const SizedBox(height: Grid.sm),
            FilledButton(
              key: const Key('token-session-retry'),
              onPressed: () => unawaited(controller.retryNow()),
              child: const Text('Retry now'),
            ),
            const SizedBox(height: Grid.xxs),
            TextButton(
              key: const Key('token-session-sign-in-again'),
              onPressed: () => unawaited(controller.signIn()),
              child: const Text('Sign in again'),
            ),
            const SizedBox(height: Grid.md),
            const CommunityRecoveryActions(),
          ],
        ),
      ),
    );
  }
}
