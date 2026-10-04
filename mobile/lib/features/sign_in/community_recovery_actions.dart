import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/community/community.dart';
import '../../shared/community/community_provider.dart';
import '../../shared/community/community_removal.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/modal_presentation.dart';

/// Ways out of a community whose session cannot be used: switch to another
/// stored community, or remove this one (AGENTS.md rule 6).
///
/// Shown by the session recovery view and the locked sign-in page so a
/// stalled keychain or a dead relay never strands the user.
class CommunityRecoveryActions extends HookConsumerWidget {
  const CommunityRecoveryActions({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
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
      final confirmed = await showBuzzDialog<bool>(
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
