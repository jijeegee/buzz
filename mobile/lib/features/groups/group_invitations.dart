import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/profile/user_cache_provider.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/frosted_app_bar.dart';
import '../../shared/widgets/frosted_scaffold.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../channels/channel_detail_page.dart';
import '../channels/group_avatar.dart';
import 'group_preview.dart';
import 'groups_api.dart';

/// Shown when an invitation can no longer be accepted.
const invitationNoLongerValidMessage = 'This invitation is no longer valid.';

/// My pending group invitations. Loaded when first watched (app start, as
/// Chats is the first tab); [refresh] runs on resume and when Chats opens.
/// There is no push for invitations yet.
class GroupInvitationsNotifier extends AsyncNotifier<List<GroupInvitation>> {
  int _serial = 0;

  @override
  Future<List<GroupInvitation>> build() {
    final api = ref.watch(groupsApiProvider);
    _serial++;
    return api.invitations();
  }

  /// Refetches; a failure keeps the list already shown.
  Future<void> refresh() async {
    final serial = ++_serial;
    try {
      final invitations = await ref.read(groupsApiProvider).invitations();
      if (!ref.mounted || serial != _serial) return;
      state = AsyncData(invitations);
    } catch (error, stackTrace) {
      if (!ref.mounted || serial != _serial) return;
      if (!state.hasValue) state = AsyncError(error, stackTrace);
    }
  }

  /// Drops [id] from the list shown.
  void remove(String id) {
    final current = state.value;
    if (current == null) return;
    state = AsyncData([
      for (final invitation in current)
        if (invitation.id != id) invitation,
    ]);
  }

  /// Accepts [invitation] and returns the group id. A 404/409 removes it
  /// and rethrows so the caller can say it is no longer valid.
  Future<String> accept(GroupInvitation invitation) async {
    try {
      final channelId = await ref
          .read(groupsApiProvider)
          .acceptInvitation(invitation.id);
      remove(invitation.id);
      return channelId;
    } on GroupsApiException catch (error) {
      if (error.isConflict || error.isNotFound) remove(invitation.id);
      rethrow;
    }
  }

  Future<void> decline(GroupInvitation invitation) async {
    try {
      await ref.read(groupsApiProvider).declineInvitation(invitation.id);
    } on GroupsApiException catch (error) {
      if (!error.isNotFound) rethrow;
    }
    remove(invitation.id);
  }
}

final groupInvitationsProvider =
    AsyncNotifierProvider<GroupInvitationsNotifier, List<GroupInvitation>>(
      GroupInvitationsNotifier.new,
    );

/// "Invitations · N" at the top of Chats; nothing when there are none.
class GroupInvitationsRow extends ConsumerWidget {
  const GroupInvitationsRow({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final count = ref.watch(groupInvitationsProvider).value?.length ?? 0;
    if (count == 0) return const SizedBox.shrink();
    return Material(
      color: Colors.transparent,
      child: InkWell(
        key: const ValueKey('chats-invitations-row'),
        onTap: () => Navigator.of(context).push(
          MaterialPageRoute<void>(
            builder: (_) => const GroupInvitationsPage(),
          ),
        ),
        child: Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: Grid.gutter,
            vertical: Grid.twelve,
          ),
          child: Row(
            children: [
              Container(
                width: 40,
                height: 40,
                decoration: BoxDecoration(
                  color: context.colors.primaryContainer,
                  shape: BoxShape.circle,
                ),
                child: Icon(
                  LucideIcons.mail,
                  size: 20,
                  color: context.colors.onPrimaryContainer,
                ),
              ),
              const SizedBox(width: Grid.twelve),
              Expanded(
                child: Text(
                  'Invitations · $count',
                  style: context.textTheme.bodyLarge?.copyWith(
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              Icon(
                LucideIcons.chevronRight,
                size: 18,
                color: context.colors.onSurfaceVariant,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The list of my pending invitations, each with Join and Decline.
class GroupInvitationsPage extends HookConsumerWidget {
  const GroupInvitationsPage({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final invitationsAsync = ref.watch(groupInvitationsProvider);
    final invitations = invitationsAsync.value;
    final inviters = {
      for (final invitation in invitations ?? const <GroupInvitation>[])
        invitation.inviter,
    }.toList()..sort();
    final users = ref.watch(userCacheProvider);
    useEffect(() {
      if (inviters.isNotEmpty) {
        unawaited(ref.read(userCacheProvider.notifier).preload(inviters));
      }
      return null;
    }, [inviters.join('|')]);
    useEffect(() {
      unawaited(
        Future<void>.microtask(
          ref.read(groupInvitationsProvider.notifier).refresh,
        ),
      );
      return null;
    }, const []);

    final Widget body;
    if (invitations == null) {
      body = invitationsAsync.hasError
          ? Center(
              child: TextButton(
                onPressed: () => unawaited(
                  ref.read(groupInvitationsProvider.notifier).refresh(),
                ),
                child: const Text('Try again'),
              ),
            )
          : const Center(child: KuzzLoadingIndicator());
    } else if (invitations.isEmpty) {
      body = Center(
        child: Text(
          'No invitations',
          key: const ValueKey('invitations-empty'),
          style: context.textTheme.bodyMedium?.copyWith(
            color: context.colors.onSurfaceVariant,
          ),
        ),
      );
    } else {
      body = ListView.builder(
        padding: EdgeInsets.only(
          top: frostedAppBarHeight(context) + Grid.xxs,
          bottom: MediaQuery.paddingOf(context).bottom + Grid.xs,
        ),
        itemCount: invitations.length,
        itemBuilder: (context, index) {
          final invitation = invitations[index];
          final name = users[invitation.inviter]?.displayName?.trim();
          return GroupInvitationTile(
            key: ValueKey('invitation-${invitation.id}'),
            invitation: invitation,
            inviterName: name == null || name.isEmpty ? null : name,
          );
        },
      );
    }

    return FrostedScaffold(
      appBar: const FrostedAppBar(title: Text('Invitations')),
      body: body,
    );
  }
}

/// One invitation: group avatar and name, "Name invited you · N members",
/// Join and Decline.
class GroupInvitationTile extends HookConsumerWidget {
  const GroupInvitationTile({
    super.key,
    required this.invitation,
    required this.inviterName,
  });

  final GroupInvitation invitation;

  /// Inviter's display name; null while unknown.
  final String? inviterName;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final busy = useState(false);
    final group = invitation.group;

    void say(String message) => ScaffoldMessenger.maybeOf(
      context,
    )?.showSnackBar(SnackBar(content: Text(message)));

    Future<void> join() async {
      if (busy.value) return;
      final navigator = Navigator.of(context);
      busy.value = true;
      try {
        final channelId = await ref
            .read(groupInvitationsProvider.notifier)
            .accept(invitation);
        final channel = await joinedChannel(ref, channelId, group);
        if (!navigator.mounted) return;
        await navigator.push(
          MaterialPageRoute<void>(
            builder: (_) => ChannelDetailPage(channel: channel),
          ),
        );
      } on GroupsApiException catch (error) {
        if (error.isConflict || error.isNotFound) {
          say(invitationNoLongerValidMessage);
        } else {
          say('Couldn’t join group. Try again.');
        }
      } catch (_) {
        say('Couldn’t join group. Try again.');
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    Future<void> decline() async {
      if (busy.value) return;
      busy.value = true;
      try {
        await ref.read(groupInvitationsProvider.notifier).decline(invitation);
      } catch (_) {
        say('Couldn’t decline. Try again.');
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    final who = inviterName ?? 'Someone';
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Grid.gutter,
        vertical: Grid.xxs,
      ),
      child: Row(
        children: [
          GroupAvatar(channel: group.toChannel(), size: 44),
          const SizedBox(width: Grid.twelve),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  group.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: context.textTheme.bodyLarge?.copyWith(
                    fontWeight: FontWeight.w600,
                  ),
                ),
                Text(
                  '$who invited you · ${memberCountLabel(group.memberCount)}',
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: context.textTheme.bodySmall?.copyWith(
                    color: context.colors.onSurfaceVariant,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: Grid.xxs),
          TextButton(
            key: ValueKey('invitation-decline-${invitation.id}'),
            onPressed: busy.value ? null : () => unawaited(decline()),
            child: const Text('Decline'),
          ),
          FilledButton(
            key: ValueKey('invitation-join-${invitation.id}'),
            onPressed: busy.value ? null : () => unawaited(join()),
            child: const Text('Join'),
          ),
        ],
      ),
    );
  }
}
