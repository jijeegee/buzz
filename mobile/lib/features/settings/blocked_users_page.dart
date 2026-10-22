import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/profile/user_cache_provider.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/bee_refresh_indicator.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../people/blocked_users_provider.dart';
import '../people/person_actions.dart';
import '../people/person_row.dart';

/// Settings → Blocked users: everyone I blocked, each with Unblock.
class BlockedUsersPage extends HookConsumerWidget {
  const BlockedUsersPage({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final blocked = ref.watch(blockedUsersProvider);
    final ids = blocked.value?.toList() ?? const <String>[];
    final cache = ref.watch(userCacheProvider);

    useEffect(() {
      if (ids.isNotEmpty) {
        ref.read(userCacheProvider.notifier).preload(ids);
      }
      return null;
    }, [Object.hashAllUnordered(ids)]);

    final sorted = [...ids]
      ..sort(
        (a, b) => personName(
          cache[a],
        ).toLowerCase().compareTo(personName(cache[b]).toLowerCase()),
      );

    // Pull to refresh: blocks changed on another device.
    Future<void> refresh() => ref.read(blockedUsersProvider.notifier).refresh();

    final Widget body;
    if (blocked.isLoading && !blocked.hasValue) {
      body = const Center(
        child: KuzzLoadingIndicator(
          size: 32,
          semanticLabel: 'Loading blocked users',
        ),
      );
    } else if (blocked.hasError && !blocked.hasValue) {
      body = Center(
        child: Padding(
          padding: const EdgeInsets.all(Grid.sm),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                'Couldn’t load blocked users',
                key: const Key('blocked-users-error'),
                style: context.textTheme.bodyMedium,
              ),
              const SizedBox(height: Grid.xs),
              FilledButton(
                key: const Key('blocked-users-retry'),
                onPressed: () => ref.invalidate(blockedUsersProvider),
                child: const Text('Try again'),
              ),
            ],
          ),
        ),
      );
    } else if (sorted.isEmpty) {
      body = BeeRefreshIndicator(
        onRefresh: refresh,
        child: LayoutBuilder(
          builder: (context, constraints) => SingleChildScrollView(
            physics: const AlwaysScrollableScrollPhysics(),
            child: SizedBox(
              height: constraints.maxHeight,
              child: Center(
                child: Text(
                  'You haven’t blocked anyone',
                  key: const Key('blocked-users-empty'),
                  style: context.textTheme.bodyMedium?.copyWith(
                    color: context.colors.onSurfaceVariant,
                  ),
                ),
              ),
            ),
          ),
        ),
      );
    } else {
      body = BeeRefreshIndicator(
        onRefresh: refresh,
        child: ListView(
          physics: const AlwaysScrollableScrollPhysics(),
          children: [
            for (final pk in sorted)
              _BlockedRow(key: ValueKey('blocked-$pk'), pubkey: pk),
          ],
        ),
      );
    }

    return Scaffold(
      appBar: AppBar(title: const Text('Blocked users')),
      body: SafeArea(child: body),
    );
  }
}

class _BlockedRow extends HookConsumerWidget {
  const _BlockedRow({super.key, required this.pubkey});

  final String pubkey;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final profile = watchPersonProfile(ref, pubkey);
    final name = personName(profile);
    final busy = useState(false);
    return ListTile(
      contentPadding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
      leading: PersonAvatar(name: name, avatarUrl: profile?.avatarUrl),
      title: Text(name, maxLines: 1, overflow: TextOverflow.ellipsis),
      trailing: OutlinedButton(
        key: Key('unblock-$pubkey'),
        onPressed: busy.value
            ? null
            : () async {
                busy.value = true;
                await unblockWithFeedback(
                  context,
                  ref,
                  pubkey: pubkey,
                  name: name,
                );
                if (context.mounted) busy.value = false;
              },
        child: Text('Unblock', semanticsLabel: 'Unblock $name'),
      ),
    );
  }
}
