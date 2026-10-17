import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/lucide_star_icon.dart';
import '../channels/channel_detail_page.dart';
import '../channels/channel_management_provider.dart';
import 'blocked_users_provider.dart';
import 'friends_provider.dart';
import 'people_favorites.dart';
import 'person_actions.dart';
import 'person_row.dart';

/// One person from People: Message, favorite, add or remove as a friend,
/// block and report. Matches desktop's People profile pane.
class PersonPage extends HookConsumerWidget {
  const PersonPage({super.key, required this.pubkey, this.summary});

  final String pubkey;

  /// What the server told us about them, before their profile loads.
  final PersonSummary? summary;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final pk = pubkey.toLowerCase();
    final me = ref.watch(currentPubkeyProvider)?.toLowerCase();
    final isSelf = me == pk;
    final profile = watchPersonProfile(ref, pk);
    final name = personName(profile, summary);
    final avatarUrl = profile?.avatarUrl ?? summary?.avatarUrl;
    final username = summary?.username;
    final isFavorite = ref.watch(
      peopleFavoritesProvider.select((ids) => ids.contains(pk)),
    );
    final friends = ref.watch(friendIdsProvider);
    final isFriend = friends.value?.contains(pk) ?? false;
    final isBlocked = ref.watch(
      blockedPubkeysProvider.select((ids) => ids.contains(pk)),
    );
    final hasAccount = ref.watch(accountOriginProvider) != null;
    final busy = useState(false);
    final opening = useState(false);

    useEffect(() {
      ref.read(userCacheProvider.notifier).preload([pk]);
      return null;
    }, [pk]);

    void showError(String message) {
      if (!context.mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(message)));
    }

    Future<void> message() async {
      if (opening.value) return;
      opening.value = true;
      try {
        final channel = await ref
            .read(channelActionsProvider)
            .openDm(pubkeys: [pk]);
        if (!context.mounted) return;
        await Navigator.of(context).push(
          MaterialPageRoute<void>(
            builder: (_) => ChannelDetailPage(channel: channel),
          ),
        );
      } catch (error) {
        showError('Couldn’t open the conversation: $error');
      } finally {
        if (context.mounted) opening.value = false;
      }
    }

    Future<void> toggleFriend() async {
      if (busy.value) return;
      busy.value = true;
      try {
        final controller = ref.read(friendsControllerProvider);
        await (isFriend ? controller.remove(pk) : controller.add(pk));
      } catch (error) {
        showError(
          isFriend
              ? 'Couldn’t remove friend: $error'
              : 'Couldn’t add friend: $error',
        );
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    void toggleFavorite() {
      final favorites = ref.read(peopleFavoritesProvider.notifier);
      if (isFavorite) {
        favorites.remove(pk);
      } else if (!favorites.add(pk)) {
        showError(peopleFavoritesFullMessage);
      }
    }

    return Scaffold(
      appBar: AppBar(title: const Text('Profile')),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(Grid.gutter),
          children: [
            Center(
              child: PersonAvatar(name: name, avatarUrl: avatarUrl, radius: 48),
            ),
            const SizedBox(height: Grid.xs),
            Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Flexible(
                  child: Text(
                    name,
                    key: const Key('person-name'),
                    textAlign: TextAlign.center,
                    style: context.textTheme.headlineSmall?.copyWith(
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ),
                if (!isSelf)
                  IconButton(
                    key: const Key('person-favorite'),
                    tooltip: isFavorite
                        ? 'Remove from favorites'
                        : 'Add to favorites',
                    isSelected: isFavorite,
                    onPressed: toggleFavorite,
                    icon: LucideStarIcon(
                      color: isFavorite
                          ? context.colors.primary
                          : context.colors.onSurfaceVariant,
                      filled: isFavorite,
                    ),
                  ),
              ],
            ),
            if (username != null)
              Text(
                '@$username',
                textAlign: TextAlign.center,
                style: context.textTheme.bodyMedium?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
            if (profile?.about?.trim() case final about? when about.isNotEmpty)
              Padding(
                padding: const EdgeInsets.only(top: Grid.xxs),
                child: Text(
                  about,
                  textAlign: TextAlign.center,
                  style: context.textTheme.bodyMedium?.copyWith(
                    color: context.colors.onSurfaceVariant,
                  ),
                ),
              ),
            if (!isSelf) ...[
              const SizedBox(height: Grid.sm),
              Row(
                children: [
                  Expanded(
                    child: FilledButton.icon(
                      key: const Key('person-message'),
                      onPressed: opening.value
                          ? null
                          : () => unawaited(message()),
                      icon: const Icon(LucideIcons.messageSquare, size: 18),
                      label: const Text('Message'),
                    ),
                  ),
                  const SizedBox(width: Grid.xxs),
                  Expanded(
                    child: OutlinedButton.icon(
                      key: const Key('person-friend'),
                      onPressed:
                          busy.value ||
                              friends.isLoading ||
                              (isBlocked && !isFriend)
                          ? null
                          : () => unawaited(toggleFriend()),
                      icon: Icon(
                        isFriend ? LucideIcons.userMinus : LucideIcons.userPlus,
                        size: 18,
                      ),
                      label: Text(isFriend ? 'Remove friend' : 'Add friend'),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: Grid.sm),
              const Divider(height: 1),
              if (hasAccount)
                ListTile(
                  key: const Key('person-block'),
                  contentPadding: EdgeInsets.zero,
                  leading: Icon(
                    LucideIcons.ban,
                    color: isBlocked ? null : context.colors.error,
                  ),
                  title: Text(
                    isBlocked ? 'Unblock' : 'Block',
                    style: isBlocked
                        ? null
                        : TextStyle(color: context.colors.error),
                  ),
                  onTap: () => unawaited(
                    isBlocked
                        ? unblockWithFeedback(
                            context,
                            ref,
                            pubkey: pk,
                            name: name,
                          )
                        : confirmAndBlock(context, ref, pubkey: pk, name: name),
                  ),
                ),
              ListTile(
                key: const Key('person-report'),
                contentPadding: EdgeInsets.zero,
                leading: Icon(LucideIcons.flag, color: context.colors.error),
                title: Text(
                  'Report',
                  style: TextStyle(color: context.colors.error),
                ),
                onTap: () =>
                    unawaited(showReportSheet(context, pubkey: pk, name: name)),
              ),
            ],
          ],
        ),
      ),
    );
  }
}
