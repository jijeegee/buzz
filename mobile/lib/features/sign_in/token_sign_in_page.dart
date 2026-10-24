import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../shared/auth/auth.dart';
import '../../shared/auth/google_key_backup.dart';
import '../../shared/auth/default_community.dart';
import '../../shared/auth/token/token.dart';
import '../../shared/kuzz_links.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import 'community_recovery_actions.dart';

/// "Sign in with Google" for relays with centralized token auth.
///
/// With a built-in server (`hasBuiltInServerProvider`, always on in the app)
/// the page signs in to that server: there is no address field. Only without
/// it (test harness) does the user first enter a relay URL, whose NIP-11
/// document decides whether token sign-in is offered. With [lockedOrigin]
/// (an existing token community whose session ended) the page goes straight
/// to the sign-in button.
class TokenSignInPage extends HookConsumerWidget {
  const TokenSignInPage({
    super.key,
    this.lockedOrigin,
    this.backupOrigin,
    this.defaultCommunity = false,
    this.resumeSignedOut = false,
    this.connectGoogle = false,
  });

  /// Ordinary first-device entry. Every page without [lockedOrigin] or
  /// [backupOrigin] now uses the built-in server when there is one, so this
  /// only documents the caller's intent.
  final bool defaultCommunity;
  final bool resumeSignedOut;

  /// Link/recover a signed identity from Settings, separate from token mode.
  final String? backupOrigin;

  /// Opened from Settings to connect a signed-in account to Google. Pops
  /// with `true` once the account is connected.
  final bool connectGoogle;

  /// Canonical origin of an existing token community to sign back in to.
  final String? lockedOrigin;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = context.l10n;
    final urlController = useTextEditingController();
    // The built-in server replaces manual address entry everywhere.
    final configuredOrigin = ref.watch(defaultCommunityOriginProvider);
    final hasBuiltInServer = configuredOrigin != null;
    final initialOrigin = lockedOrigin ?? backupOrigin ?? configuredOrigin;
    final origin = useState<String?>(initialOrigin);
    // A locked token community keeps its token session; the built-in server
    // only stands in for a missing address.
    final keyBackup = useState(
      backupOrigin != null ||
          (lockedOrigin == null && configuredOrigin != null),
    );
    final dualMode = useState(false);
    // The server offers Sign in with Apple (its NIP-11 provider list).
    final appleOffered = useState(false);
    final fixedOrigin = initialOrigin != null;
    final checking = useState(false);
    final completing = useState(false);
    final message = useState<String?>(null);
    // The Google account chosen belongs to another Kuzz account: the way
    // forward is choosing a different Google account (the relay always asks
    // Google for the account chooser), never switching identities.
    final accountMismatch = useState(false);
    // Fences relay checks: only the newest one may write its result.
    final checkGeneration = useRef(0);
    final attemptGeneration = useRef(0);
    final attemptBusy = useRef(false);
    useEffect(() {
      return () {
        attemptGeneration.value++;
      };
    }, const []);
    // A fixed server is never checked: read its providers once here.
    useEffect(() {
      final fixed = initialOrigin;
      if (fixed == null) return null;
      var live = true;
      fetchTokenAuthDescriptor(fixed, ref.read(authHttpClientProvider))
          .then((descriptor) {
            if (live && context.mounted) {
              appleOffered.value =
                  descriptor?.oidcProviders.contains('apple') ?? false;
            }
          })
          .catchError((Object _) {});
      return () => live = false;
    }, [initialOrigin]);

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
        message.value = l10n.signInRelayAddressInvalid;
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
        message.value = l10n.signInRelayNoGoogleSupport;
      } else if (!descriptor.supportsGoogle) {
        message.value = l10n.signInRelayNoGoogleConfigured;
      } else {
        keyBackup.value = descriptor.keyBackup;
        appleOffered.value = descriptor.oidcProviders.contains('apple');
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
          message.value = l10n.signInExistingIdentityConflict;
          return;
        }
        keyBackup.value = false;
      } catch (_) {
        if (context.mounted) {
          message.value = l10n.signInIdentityCheckFailed;
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
      if (lockedOrigin != null && !resumeSignedOut) return;
      completing.value = true;
      try {
        final controller = ref.read(
          keyBackup.value
              ? keyBackupSessionControllerProvider(sessionOrigin)
              : tokenSessionControllerProvider(sessionOrigin),
        );
        final sessionGeneration = controller.generation;
        final authAtStart = ref.read(authProvider).value;
        bool canCommit() =>
            context.mounted &&
            isCurrent?.call() != false &&
            identical(ref.read(authProvider).value, authAtStart) &&
            controller.generation == sessionGeneration &&
            controller.state.status == TokenSessionStatus.signedIn;
        if (keyBackup.value) {
          final community = await ref
              .read(googleKeyBackupServiceProvider(sessionOrigin))
              .resolve(isCurrent: canCommit);
          if (!canCommit()) return;
          await ref.read(signedCommunityAdmissionProvider)(community);
          if (!canCommit()) return;
          await ref
              .read(authProvider.notifier)
              .authenticateWithCommunity(
                community,
                isCurrent: canCommit,
                resumeSignedOut: resumeSignedOut,
              );
        } else {
          await ref
              .read(authProvider.notifier)
              .authenticateWithTokenSession(
                relayUrl: sessionOrigin,
                principalId: principal,
                isCurrent: canCommit,
                resumeSignedOut: resumeSignedOut,
              );
        }
      } catch (error) {
        if (context.mounted) {
          completing.value = false;
          accountMismatch.value =
              error is KeyBackupException && error.isAccountMismatch;
          message.value = error is KeyBackupException
              ? error.message
              : l10n.signInFinishFailed;
        }
        return;
      }
      if (!context.mounted) return;
      completing.value = false;
      // Opened on top of onboarding: the authenticated home is underneath.
      unawaited(Navigator.of(context).maybePop(connectGoogle ? true : null));
    }

    Future<void> signIn({String provider = 'google'}) async {
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
        accountMismatch.value = false;
        if (keyBackup.value) {
          try {
            await ref
                .read(googleKeyBackupServiceProvider(sessionOrigin))
                .checkLocalCompatibility();
            if (!context.mounted) return;
          } on KeyBackupException catch (error) {
            if (context.mounted) {
              accountMismatch.value = error.isAccountMismatch;
              message.value = error.message;
            }
            return;
          }
        }
        final controller = ref.read(
          keyBackup.value
              ? keyBackupSessionControllerProvider(sessionOrigin)
              : tokenSessionControllerProvider(sessionOrigin),
        );
        final signedIn = await controller.signIn(
          provider: provider,
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
            !resumeSignedOut &&
            lockedOrigin == null &&
            status == TokenSessionStatus.signedIn
        ? session?.principalId
        : null;

    return Scaffold(
      appBar: lockedOrigin == null && Navigator.of(context).canPop()
          ? AppBar(
              title: Text(
                connectGoogle
                    ? l10n.signInGoogleAccountTitle
                    : l10n.signInTitle,
              ),
            )
          : null,
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(Grid.sm),
          children: [
            const SizedBox(height: Grid.lg),
            Text(
              resumeSignedOut
                  ? l10n.signInSignInAgain
                  : connectGoogle
                  ? l10n.settingsProtectAccountSection
                  : keyBackup.value
                  ? l10n.signInWelcome
                  : dualMode.value
                  ? l10n.signInExistingAccountTitle
                  : lockedOrigin == null
                  ? l10n.signInToRelayTitle
                  : l10n.signInSignedOutTitle,
              style: context.textTheme.headlineSmall,
            ),
            const SizedBox(height: Grid.xxs),
            Text(
              resumeSignedOut
                  ? hasBuiltInServer
                        ? l10n.signInResumeBody
                        : l10n.signInResumeBodyCommunities
                  : connectGoogle
                  ? l10n.signInConnectGoogleBody
                  : keyBackup.value
                  ? l10n.signInWelcomeBody
                  : lockedOrigin == null
                  ? dualMode.value
                        ? l10n.signInExistingAccountBody
                        : l10n.signInToRelayBody
                  : hasBuiltInServer
                  ? l10n.signInSessionEndedBody
                  : l10n.signInSessionEndedOnRelayBody(lockedOrigin!),
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
                decoration: InputDecoration(
                  labelText: l10n.signInRelayAddressLabel,
                  hintText: 'relay.example.com',
                ),
              ),
              const SizedBox(height: Grid.xs),
              if (origin.value == null)
                OutlinedButton(
                  key: const Key('token-sign-in-check'),
                  onPressed: busy ? null : () => unawaited(checkRelay()),
                  child: Text(l10n.signInContinue),
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
                child: Text(l10n.signInContinueSaved),
              )
            else if (origin.value != null)
              FilledButton(
                key: const Key('token-sign-in-google'),
                onPressed: busy ? null : () => unawaited(signIn()),
                child: Text(
                  accountMismatch.value
                      ? l10n.signInUseDifferentGoogle
                      : resumeSignedOut || connectGoogle
                      ? l10n.settingsContinueWithGoogle
                      : l10n.signInWithGoogle,
                ),
              ),
            if (restoredPrincipal == null &&
                origin.value != null &&
                appleOffered.value &&
                !connectGoogle) ...[
              const SizedBox(height: Grid.xxs),
              OutlinedButton(
                key: const Key('token-sign-in-apple'),
                onPressed: busy
                    ? null
                    : () => unawaited(signIn(provider: 'apple')),
                child: Text(l10n.signInWithApple),
              ),
            ],
            if (restoredPrincipal == null && origin.value != null)
              const _TermsLine(),
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
                      ? l10n.signInUseExistingTokenAccount
                      : l10n.signInUseGoogleKeyRecovery,
                ),
              ),
            if (busy) ...[
              const SizedBox(height: Grid.sm),
              Center(
                child: KuzzLoadingIndicator(
                  size: 32,
                  semanticLabel: l10n.signInSigningIn,
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
            // A resume that fails (e.g. a different Google account) must
            // still leave a way out (AGENTS.md rule 6). With the built-in
            // server an already signed-out user's way out is the Google
            // button above; there is no other server to move to.
            if (lockedOrigin != null && !resumeSignedOut ||
                resumeSignedOut && errorText != null && !hasBuiltInServer) ...[
              const SizedBox(height: Grid.md),
              const CommunityRecoveryActions(),
            ],
          ],
        ),
      ),
    );
  }
}

/// "By continuing you agree to the Terms of Service", opening the terms.
class _TermsLine extends StatelessWidget {
  const _TermsLine();

  @override
  Widget build(BuildContext context) {
    final color = context.colors.onSurfaceVariant;
    // One sentence with the link text inside it, so languages can move it.
    final terms = context.l10n.signInTermsOfService;
    final sentence = context.l10n.signInTermsAgreement(terms);
    final linkStart = sentence.indexOf(terms);
    final before = linkStart < 0 ? sentence : sentence.substring(0, linkStart);
    final after = linkStart < 0
        ? ''
        : sentence.substring(linkStart + terms.length);
    return Semantics(
      link: true,
      child: TextButton(
        key: const Key('token-sign-in-terms'),
        onPressed: () => unawaited(
          launchUrl(
            Uri.parse(kuzzTermsUrl),
            mode: LaunchMode.externalApplication,
          ).catchError((Object _) => false),
        ),
        child: Text.rich(
          TextSpan(
            text: before,
            children: [
              if (linkStart >= 0)
                TextSpan(
                  text: terms,
                  style: const TextStyle(decoration: TextDecoration.underline),
                ),
              if (after.isNotEmpty) TextSpan(text: after),
            ],
          ),
          textAlign: TextAlign.center,
          style: context.textTheme.bodySmall?.copyWith(color: color),
        ),
      ),
    );
  }
}
