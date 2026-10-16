import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/auth_provider.dart';
import '../../shared/auth/default_community.dart';
import '../../shared/auth/token/token.dart';
import '../../shared/community/community.dart';
import '../../shared/community/community_provider.dart';
import '../../shared/community/community_removal.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/modal_presentation.dart';

/// Ways out of a community whose session cannot be used (AGENTS.md rule 6).
///
/// With the built-in server there is nothing to switch to or remove, so the
/// way out is "Sign out" on this device only: no network is needed (the
/// server is often what is unreachable here), the relay revocation is a
/// background best effort, and accounts, keys and messages stay on this
/// device. It is offered only while there is a signed-in session to end.
/// "Retry" and "Sign in again" with Google are the host screen's own,
/// always-shown buttons (the session recovery view's "Retry now" and
/// "Sign in again", the locked sign-in page's Google button). Without a built-in server (test harness) this offers the
/// upstream switch to another stored community or removal of this one.
///
/// Shown by the session recovery view and the locked sign-in page so a
/// stalled keychain or a dead relay never strands the user.
class CommunityRecoveryActions extends HookConsumerWidget {
  const CommunityRecoveryActions({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    if (ref.watch(hasBuiltInServerProvider)) {
      return const _SignOutRecoveryAction();
    }
    final active = ref.watch(activeCommunityProvider).value;
    final communities = ref.watch(communityListProvider).value ?? const [];
    final busy = useState(false);
    final error = useState<String?>(null);
    final others = [
      for (final community in communities)
        if (community.id != active?.id) community,
    ];

    Future<void> run(Future<void> Function() action) async {
      busy.value = true;
      error.value = null;
      try {
        await action();
      } catch (failure) {
        if (context.mounted) error.value = '$failure';
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    Future<void> remove(Community community) async {
      final confirmed = await showKuzzDialog<bool>(
        context: context,
        builder: (dialogContext) => AlertDialog(
          title: Text('Remove ${community.name}?'),
          content: const Text(
            'This community and its sign-in are removed from this device.',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(dialogContext).pop(false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              key: const Key('community-recovery-remove-confirm'),
              onPressed: () => Navigator.of(dialogContext).pop(true),
              style: FilledButton.styleFrom(
                backgroundColor: dialogContext.colors.error,
              ),
              child: const Text('Remove'),
            ),
          ],
        ),
      );
      if (confirmed != true || !context.mounted) return;
      final notifier = ref.read(communityListProvider.notifier);
      await run(() async {
        await removeCommunityWithRelaySignOut(
          context,
          ({required deviceOnly}) =>
              notifier.removeCommunity(community.id, deviceOnly: deviceOnly),
        );
      });
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (others.isNotEmpty) ...[
          Text(
            'Switch community',
            style: context.textTheme.titleSmall?.copyWith(
              color: context.colors.onSurfaceVariant,
            ),
          ),
          const SizedBox(height: Grid.xxs),
          for (final community in others)
            OutlinedButton(
              key: Key('community-recovery-switch-${community.id}'),
              onPressed: busy.value
                  ? null
                  : () => unawaited(
                      run(
                        () => ref
                            .read(communityListProvider.notifier)
                            .switchCommunity(community.id),
                      ),
                    ),
              child: Text(community.name),
            ),
          const SizedBox(height: Grid.xs),
        ],
        if (active != null)
          TextButton(
            key: const Key('community-recovery-remove'),
            onPressed: busy.value ? null : () => unawaited(remove(active)),
            style: TextButton.styleFrom(foregroundColor: context.colors.error),
            child: const Text('Remove this community'),
          ),
        if (error.value != null)
          Semantics(
            liveRegion: true,
            child: Text(
              'Could not complete that: ${error.value}',
              key: const Key('community-recovery-error'),
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.error,
              ),
            ),
          ),
      ],
    );
  }
}

/// Whether the active community has a signed-in session for "Sign out" to
/// end: not signed out, and for a token community a stored or live token
/// session (not `signedOut`, `signingIn` or still `restoring`).
final _hasSignedInSessionProvider = Provider<bool>((ref) {
  final active = ref.watch(activeCommunityProvider).value;
  if (active == null || active.signedOut) return false;
  final origin = ref.watch(activeTokenOriginProvider);
  if (origin == null) return true;
  return switch (ref.watch(tokenSessionProvider(origin)).status) {
    TokenSessionStatus.signedIn ||
    TokenSessionStatus.retrying ||
    TokenSessionStatus.stalled => true,
    TokenSessionStatus.restoring ||
    TokenSessionStatus.signingIn ||
    TokenSessionStatus.signedOut => false,
  };
});

class _SignOutRecoveryAction extends HookConsumerWidget {
  const _SignOutRecoveryAction();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final busy = useState(false);
    final error = useState<String?>(null);
    final hasSession = ref.watch(_hasSignedInSessionProvider);

    Future<void> signOut() async {
      if (busy.value) return;
      busy.value = true;
      error.value = null;
      try {
        // On this device only: never waits on the server. Ends sessions
        // only; accounts, keys and messages stay here.
        await ref.read(authProvider.notifier).signOut(deviceOnly: true);
      } catch (_) {
        if (context.mounted) {
          error.value = 'Couldn’t sign out on this device. Try again.';
        }
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    if (!hasSession && !busy.value && error.value == null) {
      return const SizedBox.shrink();
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (hasSession || busy.value)
          TextButton(
            key: const Key('community-recovery-sign-out'),
            onPressed: busy.value ? null : () => unawaited(signOut()),
            child: Text(busy.value ? 'Signing out…' : 'Sign out'),
          ),
        if (error.value != null)
          Semantics(
            liveRegion: true,
            child: Text(
              error.value!,
              key: const Key('community-recovery-error'),
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.error,
              ),
            ),
          ),
      ],
    );
  }
}
