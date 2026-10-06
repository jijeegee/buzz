import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/auth.dart';
import '../../shared/auth/google_key_backup.dart';
import '../../shared/auth/default_community.dart';
import '../../shared/auth/token/token.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/buzz_loading_indicator.dart';
import 'community_recovery_actions.dart';

/// "Sign in with Google" for relays with centralized token auth.
///
/// Without [lockedOrigin] the user first enters a relay URL; the relay's
/// NIP-11 document decides whether token sign-in is offered. With
/// [lockedOrigin] (an existing token community whose session ended) the page
/// goes straight to the sign-in button.
class TokenSignInPage extends HookConsumerWidget {
  const TokenSignInPage({
    super.key,
    this.lockedOrigin,
    this.backupOrigin,
    this.defaultCommunity = false,
  });

  /// Ordinary first-device entry uses the configured custody service.
  final bool defaultCommunity;

  /// Link/recover a signed identity from Settings, separate from token mode.
  final String? backupOrigin;

  /// Canonical origin of an existing token community to sign back in to.
  final String? lockedOrigin;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final urlController = useTextEditingController();
    final configuredOrigin = defaultCommunity
        ? ref.watch(defaultCommunityOriginProvider)
        : null;
    final initialOrigin = lockedOrigin ?? backupOrigin ?? configuredOrigin;
    final origin = useState<String?>(initialOrigin);
    final keyBackup = useState(
      backupOrigin != null || configuredOrigin != null,
    );
    final dualMode = useState(false);
    final fixedOrigin = initialOrigin != null;
    final checking = useState(false);
    final completing = useState(false);
    final message = useState<String?>(null);
    // Fences relay checks: only the newest one may write its result.
    final checkGeneration = useRef(0);
    final attemptGeneration = useRef(0);
    final attemptBusy = useRef(false);
    useEffect(() {
      return () {
        attemptGeneration.value++;
      };
    }, const []);

    final session = origin.value == null
        ? null
        : ref.watch(
            keyBackup.value
                ? keyBackupSessionProvider(origin.value!)
                : tokenSessionProvider(origin.value!),
          );

    Future<void> checkRelay() async {
      final generation = ++checkGeneration.value;
      final String candidate;
      try {
        candidate = normalizeRelayOrigin(urlController.text);
      } on FormatException {
        message.value = 'Enter a relay address, like relay.example.com.';
        return;
      }
      origin.value = null;
      message.value = null;
      checking.value = true;
      String? failure;
      TokenAuthDescriptor? descriptor;
      try {
        descriptor = await fetchTokenAuthDescriptor(
          candidate,
          ref.read(authHttpClientProvider),
        );
      } on TokenAuthDetectionException catch (error) {
        failure = error.message;
      }
      if (!context.mounted || generation != checkGeneration.value) return;
      checking.value = false;
      if (failure != null) {
        message.value = failure;
      } else if (descriptor == null) {
        message.value =
            'This relay does not support Google sign-in. Join it with an '
            'invite link from a member instead.';
      } else if (!descriptor.supportsGoogle) {
        message.value = 'This relay has no Google sign-in configured.';
      } else {
        keyBackup.value = descriptor.keyBackup;
        dualMode.value =
            descriptor.keyBackup && descriptor.existingTokenAccounts;
        origin.value = candidate;
      }
    }

    Future<void> selectExistingTokenAccount() async {
      final sessionOrigin = origin.value;
      if (sessionOrigin == null) return;
      checking.value = true;
      message.value = null;
      try {
        final communities = await ref.read(communityStorageProvider).loadAll();
        if (!context.mounted) return;
        if (communities.any(
          (community) =>
              !community.tokenAuth &&
              normalizeRelayOrigin(community.relayUrl) == sessionOrigin,
        )) {
          message.value =
              'This device has a signing identity on this relay. '
              'An existing token account cannot replace it. Use Google key recovery.';
          return;
        }
        keyBackup.value = false;
      } catch (_) {
        if (context.mounted) {
          message.value = 'Could not check the saved identity. Try again.';
        }
      } finally {
        if (context.mounted) checking.value = false;
      }
    }

    /// Records the signed-in session as the active community. In locked
    /// mode the community already exists and `TokenSessionGate` owns
    /// identity changes.
    Future<void> enterCommunity(
      String sessionOrigin,
      String principal, {
      bool Function()? isCurrent,
    }) async {
      if (lockedOrigin != null) return;
      completing.value = true;
      try {
        if (keyBackup.value) {
          final controller = ref.read(
            keyBackupSessionControllerProvider(sessionOrigin),
          );
          final sessionGeneration = controller.generation;
          final authAtStart = ref.read(authProvider).value;
          bool canCommit() =>
              context.mounted &&
              isCurrent?.call() != false &&
              identical(ref.read(authProvider).value, authAtStart) &&
              controller.generation == sessionGeneration &&
              controller.state.status == TokenSessionStatus.signedIn;
          final community = await ref
              .read(googleKeyBackupServiceProvider(sessionOrigin))
              .resolve(isCurrent: canCommit);
          if (!canCommit()) return;
          await ref.read(signedCommunityAdmissionProvider)(community);
          if (!canCommit()) return;
          await ref
              .read(authProvider.notifier)
              .authenticateWithCommunity(community, isCurrent: canCommit);
        } else {
          await ref
              .read(authProvider.notifier)
              .authenticateWithTokenSession(
                relayUrl: sessionOrigin,
                principalId: principal,
              );
        }
      } catch (error) {
        if (context.mounted) {
          completing.value = false;
          message.value = error is KeyBackupException
              ? error.message
              : 'Could not save this community. Your existing identity is unchanged.';
        }
        return;
      }
      if (!context.mounted) return;
      completing.value = false;
      // Opened on top of onboarding: the authenticated home is underneath.
      unawaited(Navigator.of(context).maybePop());
    }

    Future<void> signIn() async {
      final sessionOrigin = origin.value;
      if (sessionOrigin == null) return;
      if (attemptBusy.value) return;
      attemptBusy.value = true;
      completing.value = true;
      final generation = ++attemptGeneration.value;
      bool isCurrent() =>
          context.mounted && generation == attemptGeneration.value;
      try {
        message.value = null;
        if (keyBackup.value) {
          try {
            await ref
                .read(googleKeyBackupServiceProvider(sessionOrigin))
                .checkLocalCompatibility();
            if (!context.mounted) return;
          } on KeyBackupException catch (error) {
            if (context.mounted) message.value = error.message;
            return;
          }
        }
        final controller = ref.read(
          keyBackup.value
              ? keyBackupSessionControllerProvider(sessionOrigin)
              : tokenSessionControllerProvider(sessionOrigin),
        );
        final signedIn = await controller.signIn(
          identityMode: keyBackup.value ? 'key_backup' : 'token',
        );
        final principal = controller.state.principalId;
        if (!signedIn || principal == null || !context.mounted) return;
        await enterCommunity(sessionOrigin, principal, isCurrent: isCurrent);
      } finally {
        attemptBusy.value = false;
        if (context.mounted) completing.value = false;
      }
    }

    final status = session?.status;
    final busy =
        checking.value ||
        completing.value ||
        status == TokenSessionStatus.restoring ||
        status == TokenSessionStatus.signingIn;
    final errorText = message.value ?? session?.errorMessage;
    final restoredPrincipal =
        !keyBackup.value &&
            lockedOrigin == null &&
            status == TokenSessionStatus.signedIn
        ? session?.principalId
        : null;

    return Scaffold(
      appBar: lockedOrigin == null && Navigator.of(context).canPop()
          ? AppBar(title: const Text('Sign in'))
          : null,
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(Grid.sm),
          children: [
            const SizedBox(height: Grid.lg),
            Text(
              keyBackup.value
                  ? 'Welcome to Buzz'
                  : dualMode.value
                  ? 'Existing account sign-in'
                  : lockedOrigin == null
                  ? 'Sign in to a relay'
                  : 'Signed out',
              style: context.textTheme.headlineSmall,
            ),
            const SizedBox(height: Grid.xxs),
            Text(
              keyBackup.value
                  ? 'Sign in on your first device or restore the same Buzz identity here. '
                        'If this account has no backup, your current key is linked, '
                        'or a new key is created on this device. The server encrypts '
                        'the backup, but its operator or anyone controlling your '
                        'Google account can recover your key. Signing out cannot '
                        'revoke a copied key.'
                  : lockedOrigin == null
                  ? dualMode.value
                        ? 'Return to an account created before key recovery was added. '
                              'Its existing identity and history are kept. '
                              'New accounts use Google key recovery.'
                        : 'Relays with Buzz accounts let you sign in with Google.'
                  : 'Your session on $lockedOrigin ended. Sign in again '
                        'to continue.',
              style: context.textTheme.bodyMedium?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
            const SizedBox(height: Grid.sm),
            if (!fixedOrigin) ...[
              TextField(
                key: const Key('token-sign-in-relay-url'),
                controller: urlController,
                enabled: !busy,
                keyboardType: TextInputType.url,
                autocorrect: false,
                textInputAction: TextInputAction.go,
                onChanged: (_) {
                  // An edited address invalidates the previous check.
                  checkGeneration.value += 1;
                  checking.value = false;
                  origin.value = null;
                  dualMode.value = false;
                },
                onSubmitted: (_) => unawaited(checkRelay()),
                decoration: const InputDecoration(
                  labelText: 'Relay address',
                  hintText: 'relay.example.com',
                ),
              ),
              const SizedBox(height: Grid.xs),
              if (origin.value == null)
                OutlinedButton(
                  key: const Key('token-sign-in-check'),
                  onPressed: busy ? null : () => unawaited(checkRelay()),
                  child: const Text('Continue'),
                ),
            ],
            if (restoredPrincipal != null)
              FilledButton(
                key: const Key('token-sign-in-continue'),
                onPressed: busy
                    ? null
                    : () => unawaited(
                        enterCommunity(origin.value!, restoredPrincipal),
                      ),
                child: const Text('Continue with saved sign-in'),
              )
            else if (origin.value != null)
              FilledButton(
                key: const Key('token-sign-in-google'),
                onPressed: busy ? null : () => unawaited(signIn()),
                child: const Text('Sign in with Google'),
              ),
            if (dualMode.value && origin.value != null && !fixedOrigin)
              TextButton(
                key: const Key('token-sign-in-existing-account'),
                onPressed: busy
                    ? null
                    : () {
                        if (keyBackup.value) {
                          unawaited(selectExistingTokenAccount());
                        } else {
                          message.value = null;
                          keyBackup.value = true;
                        }
                      },
                child: Text(
                  keyBackup.value
                      ? 'Use an existing token account'
                      : 'Use Google key recovery',
                ),
              ),
            if (defaultCommunity && configuredOrigin != null)
              TextButton(
                key: const Key('sign-in-custom-community'),
                onPressed: busy
                    ? null
                    : () => Navigator.of(context).push(
                        MaterialPageRoute<void>(
                          builder: (_) => const TokenSignInPage(),
                        ),
                      ),
                child: const Text('Advanced: use a custom community'),
              ),
            if (busy) ...[
              const SizedBox(height: Grid.sm),
              const Center(
                child: BuzzLoadingIndicator(
                  size: 32,
                  semanticLabel: 'Signing in',
                ),
              ),
            ],
            if (errorText != null) ...[
              const SizedBox(height: Grid.xs),
              Semantics(
                liveRegion: true,
                child: Text(
                  errorText,
                  key: const Key('token-sign-in-error'),
                  style: context.textTheme.bodySmall?.copyWith(
                    color: context.colors.error,
                  ),
                ),
              ),
            ],
            if (lockedOrigin != null) ...[
              const SizedBox(height: Grid.md),
              const CommunityRecoveryActions(),
            ],
          ],
        ),
      ),
    );
  }
}
