import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/auth/default_community.dart';
import '../../shared/auth/token/token.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/widgets/modal_presentation.dart';
import 'invite_join_provider.dart';

Future<bool?> showInviteJoinSheet(BuildContext context) {
  return showKuzzModalBottomSheet<bool>(
    context: context,
    isScrollControlled: true,
    // The route remains a normal modal while idle, but its contents install a
    // PopScope once a membership claim or starter setup begins. Keep drag and
    // the shared close affordance out of this sheet so they cannot bypass that
    // in-flight guard.
    enableDrag: false,
    showCloseButton: false,
    builder: (_) => const _InviteJoinSheetRoute(),
  );
}

class _InviteJoinSheetRoute extends ConsumerWidget {
  const _InviteJoinSheetRoute();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final isClaiming =
        ref.watch(inviteJoinProvider).status == InviteJoinStatus.claiming;
    return PopScope(canPop: !isClaiming, child: const InviteJoinSheet());
  }
}

class InviteJoinSheet extends ConsumerWidget {
  const InviteJoinSheet({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final state = ref.watch(inviteJoinProvider);
    // With the built-in server the sheet never shows a host or server name.
    final hasBuiltInServer = ref.watch(hasBuiltInServerProvider);
    final isClaiming = state.status == InviteJoinStatus.claiming;
    final isStarterSetupRecovery = state.isStarterSetupRecovery;
    final l10n = context.l10n;
    final host = state.host ?? l10n.inviteUnknownHost;
    final derivedName = state.communityName;
    final tokenOrigin = state.tokenRelayOrigin;
    final tokenStatus = tokenOrigin == null
        ? null
        : ref.watch(tokenSessionProvider(tokenOrigin)).status;
    final needsSignIn =
        tokenStatus != null && tokenStatus != TokenSessionStatus.signedIn;
    final primaryLabel = switch ((
      isClaiming,
      isStarterSetupRecovery,
      state.status,
    )) {
      (true, true, _) => l10n.inviteFinishingSetup,
      (true, false, _) => l10n.inviteJoining,
      (false, true, InviteJoinStatus.error) => l10n.inviteRetrySetup,
      (false, true, _) => l10n.inviteFinishSettingUp,
      _ when needsSignIn => l10n.inviteSignInWithGoogleToJoin,
      _ => l10n.inviteJoin,
    };

    if (state.status == InviteJoinStatus.success) {
      return _InviteJoinSuccess(
        host: host,
        communityName: derivedName,
        hasFocusChannel: state.focusChannelId != null,
        isTokenAuth: tokenOrigin != null,
        hideServer: hasBuiltInServer,
      );
    }

    return SafeArea(
      child: SingleChildScrollView(
        padding: const EdgeInsets.fromLTRB(Grid.sm, 0, Grid.sm, Grid.sm),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Icon(LucideIcons.userPlus, size: 40, color: context.colors.primary),
            const SizedBox(height: Grid.sm),
            Text(
              isStarterSetupRecovery
                  ? l10n.inviteFinishSettingUp
                  : hasBuiltInServer
                  ? l10n.inviteJoinTitle
                  : l10n.inviteJoinCommunityTitle,
              key: const Key('invite-join-title'),
              style: context.textTheme.titleLarge,
            ),
            if (!hasBuiltInServer) ...[
              const SizedBox(height: Grid.xxs),
              Text(
                l10n.inviteCheckHost,
                style: context.textTheme.bodyMedium?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
              const SizedBox(height: Grid.xs),
              Container(
                padding: const EdgeInsets.all(Grid.twelve),
                decoration: BoxDecoration(
                  color: context.colors.surfaceContainerHighest.withValues(
                    alpha: 0.7,
                  ),
                  borderRadius: BorderRadius.circular(Radii.container),
                  border: Border.all(color: context.colors.outlineVariant),
                ),
                child: Text(
                  host,
                  style: context.textTheme.titleMedium?.copyWith(
                    fontFamily: 'GeistMono',
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              if (derivedName != null && derivedName != host) ...[
                const SizedBox(height: Grid.xxs),
                Text(
                  l10n.inviteDisplayName(derivedName),
                  style: context.textTheme.bodySmall?.copyWith(
                    color: context.colors.onSurfaceVariant,
                  ),
                ),
              ],
            ],
            const SizedBox(height: Grid.sm),
            if (isStarterSetupRecovery) ...[
              Text(
                l10n.inviteStarterSetupPending,
                style: context.textTheme.bodyMedium?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
              const SizedBox(height: Grid.sm),
            ],
            if (tokenStatus != null) ...[
              Text(
                hasBuiltInServer
                    ? l10n.inviteGoogleMembership
                    : l10n.inviteGoogleMembershipCommunity,
                style: context.textTheme.bodyMedium?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
              const SizedBox(height: Grid.xxs),
              Text(
                switch (tokenStatus) {
                  TokenSessionStatus.signedIn => l10n.inviteSessionSignedIn,
                  TokenSessionStatus.signingIn => l10n.inviteSessionSigningIn,
                  TokenSessionStatus.restoring => l10n.inviteSessionChecking,
                  TokenSessionStatus.retrying || TokenSessionStatus.stalled =>
                    l10n.inviteSessionNeedsConnection,
                  TokenSessionStatus.signedOut => l10n.inviteSessionSignedOut,
                },
                style: context.textTheme.bodySmall?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
            ] else
              Text(
                l10n.inviteOnlyCopyWarning,
                style: context.textTheme.bodyMedium?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
            if (state.status == InviteJoinStatus.error &&
                state.errorMessage != null) ...[
              const SizedBox(height: Grid.sm),
              Text(
                state.errorMessage!,
                style: context.textTheme.bodySmall?.copyWith(
                  color: context.colors.error,
                ),
              ),
            ],
            const SizedBox(height: Grid.lg),
            Row(
              children: [
                Expanded(
                  child: OutlinedButton(
                    onPressed: isClaiming
                        ? null
                        : () => Navigator.of(context).pop(),
                    child: Text(l10n.commonCancel),
                  ),
                ),
                const SizedBox(width: Grid.sm),
                Expanded(
                  child: FilledButton.icon(
                    onPressed: isClaiming || state.requiresFreshInvite
                        ? null
                        : () async => ref
                              .read(inviteJoinProvider.notifier)
                              .confirmJoin(),
                    icon: isClaiming
                        ? SizedBox(
                            width: 16,
                            height: 16,
                            child: KuzzLoadingIndicator(
                              size: 16,
                              semanticLabel: isStarterSetupRecovery
                                  ? l10n.inviteFinishingSetupLabel
                                  : hasBuiltInServer
                                  ? l10n.inviteJoiningLabel
                                  : l10n.inviteJoiningCommunityLabel,
                            ),
                          )
                        : Icon(
                            needsSignIn ? LucideIcons.logIn : LucideIcons.check,
                          ),
                    label: Text(primaryLabel),
                  ),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

class _InviteJoinSuccess extends StatelessWidget {
  final String host;
  final String? communityName;
  final bool hasFocusChannel;
  final bool isTokenAuth;
  final bool hideServer;

  const _InviteJoinSuccess({
    required this.host,
    required this.hasFocusChannel,
    required this.isTokenAuth,
    required this.hideServer,
    this.communityName,
  });

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(Grid.sm, 0, Grid.sm, Grid.sm),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Icon(
              LucideIcons.circleCheck,
              size: 40,
              color: context.colors.primary,
            ),
            const SizedBox(height: Grid.sm),
            Text(
              hideServer
                  ? context.l10n.inviteJoinedTitle
                  : context.l10n.inviteJoinedCommunity(communityName ?? host),
              style: context.textTheme.titleLarge,
            ),
            if (!isTokenAuth) ...[
              const SizedBox(height: Grid.xs),
              Text(
                context.l10n.inviteOnlyCopyWarning,
                style: context.textTheme.bodyMedium?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
            ],
            const SizedBox(height: Grid.lg),
            FilledButton(
              onPressed: () => Navigator.of(context).pop(hasFocusChannel),
              child: Text(
                hasFocusChannel
                    ? context.l10n.inviteJoinedContinue
                    : context.l10n.inviteJoinedDone,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
