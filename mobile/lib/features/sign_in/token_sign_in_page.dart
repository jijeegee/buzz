import 'dart:async';

import 'package:flutter/gestures.dart';
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
import 'device_key_notice.dart';

/// "Sign in with Google" / "Sign in with Apple" for relays with centralized
/// token auth.
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
    this.lockedSession,
    this.backupOrigin,
    this.defaultCommunity = false,
    this.resumeSignedOut = false,
    this.connectGoogle = false,
    this.addAccount = false,
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

  /// Opened over a signed-in account. An account already signed in here is
  /// switched to, keeping the session it has; the attempt then ends.
  final bool addAccount;

  /// Canonical origin of an existing token community to sign back in to.
  final String? lockedOrigin;

  /// The account whose session ended, when known: the page shows why.
  final SessionKey? lockedSession;

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
    final fixedOrigin = initialOrigin != null;
    final checking = useState(false);
    final completing = useState(false);
    final message = useState<String?>(null);
    // The chosen sign-in has no Kuzz account yet: the question on screen, and
    // the sign-in waiting for its answer.
    final signup = useState<({SignupRequired prompt, String provider})?>(null);
    final signupAnswer = useRef<Completer<SignupChoice>?>(null);
    // Fences relay checks: only the newest one may write its result.
    final checkGeneration = useRef(0);
    final attemptGeneration = useRef(0);
    final attemptBusy = useRef(false);
    // The running sign-in. Its session stays in memory until the account is
    // known and takes it over; the stored accounts are never touched here.
    final attempt = useRef<TokenSessionController?>(null);
    final session = useState<TokenSessionState?>(null);

    void answerSignup(SignupChoice choice) {
      final answer = signupAnswer.value;
      signupAnswer.value = null;
      if (answer != null && !answer.isCompleted) answer.complete(choice);
    }

    void closeAttempt() {
      answerSignup(SignupChoice.close);
      final controller = attempt.value;
      attempt.value = null;
      // Handed over, this ends nothing: the attempt holds no tokens anymore.
      if (controller != null) {
        unawaited(controller.discard().whenComplete(controller.dispose));
      }
    }

    TokenSessionController startAttempt(String sessionOrigin) {
      closeAttempt();
      final controller = ref.read(tokenSignInAttemptProvider)(sessionOrigin);
      controller.addListener((next) {
        if (context.mounted && identical(attempt.value, controller)) {
          session.value = next;
        }
      });
      attempt.value = controller;
      session.value = null;
      return controller;
    }

    useEffect(() {
      return () {
        attemptGeneration.value++;
        closeAttempt();
      };
    }, const []);

    final endedSession = lockedSession == null
        ? null
        : ref.watch(tokenSessionProvider(lockedSession!));

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
        dualMode.value =
            descriptor.keyBackup && descriptor.existingTokenAccounts;
        origin.value = candidate;
      }
    }

    /// Records the signed-in session as the active community; a signing-key
    /// community also records the [provider] it signed in with (null keeps
    /// the recorded one). In locked
    /// mode `TokenSessionGate` replaces this page once the session is back.
    /// [controller] is the attempt; its account takes it over here.
    Future<void> enterCommunity(
      TokenSessionController controller,
      String sessionOrigin,
      String principal, {
      bool Function()? isCurrent,
      String? provider,
    }) async {
      completing.value = true;
      try {
        final sessionGeneration = controller.generation;
        bool canCommit() =>
            context.mounted &&
            isCurrent?.call() != false &&
            controller.generation == sessionGeneration &&
            controller.state.status == TokenSessionStatus.signedIn;
        final signedInRow = addAccount
            ? (await ref.read(communityStorageProvider).loadAll()).where((row) {
                return !row.signedOut &&
                    SessionKey.ofCommunity(row) ==
                        SessionKey(sessionOrigin, principal);
              }).firstOrNull
            : null;
        if (signedInRow != null) {
          if (!canCommit()) return;
          await ref
              .read(communityListProvider.notifier)
              .switchCommunity(signedInRow.id);
        } else if (keyBackup.value) {
          final community = await ref
              .read(googleKeyBackupServiceProvider(sessionOrigin))
              .resolve(
                session: controller,
                connect: connectGoogle,
                isCurrent: canCommit,
              );
          if (!canCommit()) return;
          final signedIn = community.copyWith(signInProvider: provider);
          await ref.read(signedCommunityAdmissionProvider)(signedIn);
          if (!canCommit()) return;
          await ref
              .read(authProvider.notifier)
              .authenticateWithCommunity(
                signedIn,
                isCurrent: canCommit,
                attempt: controller,
              );
        } else {
          await ref
              .read(authProvider.notifier)
              .authenticateWithTokenSession(
                relayUrl: sessionOrigin,
                principalId: principal,
                isCurrent: canCommit,
                attempt: controller,
              );
        }
      } catch (error) {
        if (context.mounted) {
          completing.value = false;
          message.value = error is KeyBackupException
              ? error.message
              : l10n.signInFinishFailed;
        }
        return;
      }
      if (!context.mounted) return;
      completing.value = false;
      if (lockedOrigin != null && !resumeSignedOut) return;
      // Opened on top of onboarding: the authenticated home is underneath.
      unawaited(Navigator.of(context).maybePop(connectGoogle ? true : null));
    }

    /// Puts the question on screen until [answerSignup]. Connecting Google
    /// does not ask: that account is the one on screen.
    Future<SignupChoice> confirmSignup(
      SignupRequired prompt,
      String provider,
    ) async {
      if (!context.mounted) return SignupChoice.close;
      final answer = signupAnswer.value = Completer<SignupChoice>();
      signup.value = (prompt: prompt, provider: provider);
      final choice = await answer.future;
      if (context.mounted) signup.value = null;
      return choice;
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
        ref.read(signInNoticeProvider.notifier).clear();
        final controller = startAttempt(sessionOrigin);
        final signedIn = await controller.signIn(
          provider: provider,
          identityMode: keyBackup.value ? 'key_backup' : 'token',
          confirmSignup: keyBackup.value && !connectGoogle
              ? (prompt) => confirmSignup(prompt, provider)
              : null,
        );
        final principal = controller.state.principalId;
        if (!signedIn || principal == null || !context.mounted) return;
        await enterCommunity(
          controller,
          sessionOrigin,
          principal,
          isCurrent: isCurrent,
          provider: provider,
        );
      } finally {
        attemptBusy.value = false;
        // Anything not handed over by now is abandoned: its session ends.
        closeAttempt();
        if (context.mounted) completing.value = false;
      }
    }

    final busy =
        checking.value ||
        completing.value ||
        session.value?.status == TokenSessionStatus.signingIn;
    final errorText =
        message.value ??
        session.value?.errorMessage ??
        endedSession?.errorMessage ??
        ref.watch(signInNoticeProvider);
    // Any account may sign in here; connecting an older account is Google
    // only.
    final offersApple = !connectGoogle;
    final signInWithApple = busy
        ? null
        : () => unawaited(signIn(provider: 'apple'));
    final appleLabel = resumeSignedOut
        ? l10n.signInContinueWithApple
        : l10n.signInWithApple;

    if (signup.value case (:final prompt, :final provider)?) {
      return _CreateAccountView(
        email: prompt.email,
        apple: provider == 'apple',
        onAnswer: answerSignup,
      );
    }

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
            if (origin.value != null)
              FilledButton(
                key: const Key('token-sign-in-google'),
                onPressed: busy ? null : () => unawaited(signIn()),
                child: Text(
                  resumeSignedOut || connectGoogle
                      ? l10n.settingsContinueWithGoogle
                      : l10n.signInWithGoogle,
                ),
              ),
            if (origin.value != null && offersApple) ...[
              const SizedBox(height: Grid.xxs),
              OutlinedButton(
                key: const Key('token-sign-in-apple'),
                onPressed: signInWithApple,
                child: Text(appleLabel),
              ),
            ],
            if (origin.value != null) const _TermsLine(),
            if (dualMode.value && origin.value != null && !fixedOrigin)
              TextButton(
                key: const Key('token-sign-in-existing-account'),
                onPressed: busy
                    ? null
                    : () {
                        message.value = null;
                        keyBackup.value = !keyBackup.value;
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

/// "Create your Kuzz account": the chosen sign-in has no account yet, and
/// nothing exists until "Create account". Going back stops here.
class _CreateAccountView extends StatelessWidget {
  const _CreateAccountView({
    required this.email,
    required this.apple,
    required this.onAnswer,
  });

  final String? email;
  final bool apple;
  final void Function(SignupChoice choice) onAnswer;

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) onAnswer(SignupChoice.close);
      },
      child: Scaffold(
        body: SafeArea(
          child: ListView(
            key: const Key('create-account'),
            padding: const EdgeInsets.all(Grid.sm),
            children: [
              const SizedBox(height: Grid.lg),
              Text(
                l10n.signInCreateAccountTitle,
                style: context.textTheme.headlineSmall,
              ),
              const SizedBox(height: Grid.xxs),
              Text(
                apple
                    ? l10n.signInCreateAccountBodyApple
                    : l10n.signInCreateAccountBody,
                style: context.textTheme.bodyMedium?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
              if (email case final email?) ...[
                const SizedBox(height: Grid.xs),
                Text(
                  email,
                  key: const Key('create-account-email'),
                  style: context.textTheme.titleMedium,
                ),
              ],
              const SizedBox(height: Grid.sm),
              FilledButton(
                key: const Key('create-account-confirm'),
                onPressed: () => onAnswer(SignupChoice.create),
                child: Text(l10n.signInCreateAccountConfirm),
              ),
              const SizedBox(height: Grid.xxs),
              OutlinedButton(
                key: const Key('create-account-different'),
                onPressed: () => onAnswer(SignupChoice.different),
                child: Text(l10n.signInCreateAccountDifferent),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// "By continuing you agree to the Terms of Service and Privacy Policy",
/// each link opening its page on kuzz.app.
class _TermsLine extends StatefulWidget {
  const _TermsLine();

  @override
  State<_TermsLine> createState() => _TermsLineState();
}

class _TermsLineState extends State<_TermsLine> {
  late final _termsTap = TapGestureRecognizer()
    ..onTap = () => _open(kuzzTermsUrl);
  late final _privacyTap = TapGestureRecognizer()
    ..onTap = () => _open(kuzzPrivacyUrl);

  void _open(String url) => unawaited(
    launchUrl(
      Uri.parse(url),
      mode: LaunchMode.externalApplication,
    ).catchError((Object _) => false),
  );

  @override
  void dispose() {
    _termsTap.dispose();
    _privacyTap.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    // One sentence with the link texts inside it, so languages can move them.
    final links = [
      (text: l10n.signInTermsOfService, tap: _termsTap),
      (text: l10n.signInPrivacyPolicy, tap: _privacyTap),
    ];
    final sentence = l10n.signInTermsAgreement(links[0].text, links[1].text);
    final found = [
      for (final link in links)
        if (sentence.indexOf(link.text) case final start when start >= 0)
          (start: start, link: link),
    ]..sort((a, b) => a.start.compareTo(b.start));
    const underline = TextStyle(decoration: TextDecoration.underline);
    final spans = <InlineSpan>[];
    var at = 0;
    for (final (:start, :link) in found) {
      if (start < at) continue;
      if (start > at) spans.add(TextSpan(text: sentence.substring(at, start)));
      spans.add(
        TextSpan(text: link.text, style: underline, recognizer: link.tap),
      );
      at = start + link.text.length;
    }
    if (at < sentence.length) spans.add(TextSpan(text: sentence.substring(at)));
    return Padding(
      key: const Key('token-sign-in-terms'),
      padding: const EdgeInsets.symmetric(vertical: Grid.xs),
      child: Text.rich(
        TextSpan(children: spans),
        textAlign: TextAlign.center,
        style: context.textTheme.bodySmall?.copyWith(
          color: context.colors.onSurfaceVariant,
        ),
      ),
    );
  }
}
