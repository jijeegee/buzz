import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../../shared/mentions/agent_identity_provider.dart';
import '../../../shared/relay/relay.dart';
import '../../../shared/profile/user_cache_provider.dart';
import '../../../shared/theme/theme.dart';
import '../../../shared/widgets/conversation_row_parts.dart';
import '../../../shared/widgets/modal_presentation.dart';
import '../../../shared/identity_names/identity_names_provider.dart';
import '../../profile/user_profile_sheet.dart';
import '../add_members_sheet.dart';
import '../channel.dart';
import '../channel_management_provider.dart';
import '../channel_mutes/channel_mutes_provider.dart';
import '../channel_names/channel_names_provider.dart';
import '../channel_names/channel_names_storage.dart';
import '../room_goals_switch.dart';
import 'chat_grid_avatar.dart';
import 'chat_title.dart';

/// "1 person" / "N people".
String chatPeopleLabel(int count) => count == 1 ? '1 person' : '$count people';

/// Opens Chat info for a chat of people. Returns true when I left it, so
/// the page showing the chat should close.
Future<bool?> showChatInfoSheet({
  required BuildContext context,
  required Channel channel,
}) => showKuzzModalBottomSheet<bool>(
  context: context,
  title: 'Chat info',
  isScrollControlled: true,
  showDragHandle: true,
  constraints: BoxConstraints(
    maxWidth: 640,
    maxHeight: MediaQuery.sizeOf(context).height * 0.9,
  ),
  builder: (_) => ChatInfoSheet(channel: channel),
);

/// A chat's photo, title, people and my own settings for it. Every member
/// is equal here: no owner or admin controls.
class ChatInfoSheet extends HookConsumerWidget {
  const ChatInfoSheet({super.key, required this.channel});

  final Channel channel;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final displayed = useState(channel);
    final members =
        ref.watch(channelMembersProvider(channel.id)).value ??
        const <ChannelMember>[];
    final memberPubkeys = members.isEmpty
        ? channel.participantPubkeys
        : [for (final member in members) member.pubkey.toLowerCase()];
    final shown = displayed.value.copyWith(participantPubkeys: memberPubkeys);
    final title = watchChannelTitle(ref, shown);
    final privateName = ref.watch(channelPrivateNameProvider(channel.id));
    final isMuted =
        ref.watch(channelMutesProvider).store.channels[channel.id]?.muted ==
        true;
    final count = members.isEmpty ? channel.memberCount : members.length;
    final names = watchIdentityNames(
      ref,
      memberPubkeys,
      agentPubkeys: {
        for (final member in members)
          if (member.isBot) member.pubkey.toLowerCase(),
      },
      fallbackNames: {
        for (final member in members)
          member.pubkey.toLowerCase(): ?member.displayName,
      },
    );
    final me = ref.watch(myPubkeyProvider)?.toLowerCase();
    final canAct = channel.isMember && !channel.isArchived;

    Future<void> addPeople() async {
      await showKuzzModalBottomSheet<bool>(
        context: context,
        title: 'Add people',
        isScrollControlled: true,
        showDragHandle: true,
        builder: (_) => AddChannelMembersSheet(
          channelId: channel.id,
          existingPubkeys: memberPubkeys.toSet(),
          chat: true,
        ),
      );
    }

    Future<void> rename() async {
      final next = await showKuzzDialog<String>(
        context: context,
        builder: (_) => _RenameDialog(initial: privateName ?? title),
      );
      // Saving the title unchanged sets nothing.
      if (next == null || (privateName == null && next.trim() == title)) {
        return;
      }
      ref.read(channelNamesProvider.notifier).setName(channel.id, next);
    }

    Future<void> leave() async {
      final confirmed = await showKuzzDialog<bool>(
        context: context,
        builder: (dialogContext) => AlertDialog(
          title: const Text('Leave this chat?'),
          content: const Text('You’ll stop receiving messages from it.'),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(dialogContext).pop(false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              key: const ValueKey('chat-info-leave-confirm'),
              onPressed: () => Navigator.of(dialogContext).pop(true),
              child: const Text('Leave'),
            ),
          ],
        ),
      );
      if (confirmed != true || !context.mounted) return;
      try {
        await ref.read(channelActionsProvider).leaveChannel(channel.id);
        if (context.mounted) Navigator.of(context).pop(true);
      } catch (_) {
        if (!context.mounted) return;
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('Couldn’t leave the chat. Try again.')),
        );
      }
    }

    return SafeArea(
      top: false,
      child: ListView(
        key: const ValueKey('chat-info-sheet'),
        shrinkWrap: true,
        padding: const EdgeInsets.fromLTRB(
          Grid.gutter,
          0,
          Grid.gutter,
          Grid.xs,
        ),
        children: [
          Center(child: ChatGridAvatar(memberPubkeys: memberPubkeys, size: 72)),
          const SizedBox(height: Grid.xs),
          Text(
            title,
            key: const ValueKey('chat-info-title'),
            textAlign: TextAlign.center,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: context.textTheme.titleMedium?.copyWith(
              fontWeight: FontWeight.w600,
            ),
          ),
          Text(
            chatPeopleLabel(count),
            key: const ValueKey('chat-info-count'),
            textAlign: TextAlign.center,
            style: context.textTheme.bodySmall?.copyWith(
              color: context.colors.onSurfaceVariant,
            ),
          ),
          const SizedBox(height: Grid.xs),
          if (canAct)
            ListTile(
              key: const ValueKey('chat-info-add-people'),
              contentPadding: EdgeInsets.zero,
              leading: const Icon(LucideIcons.userPlus),
              title: const Text('Add people'),
              onTap: addPeople,
            ),
          ListTile(
            key: const ValueKey('chat-info-rename'),
            contentPadding: EdgeInsets.zero,
            leading: const Icon(LucideIcons.pencil),
            title: const Text('Rename for me'),
            onTap: rename,
          ),
          if (privateName != null)
            ListTile(
              key: const ValueKey('chat-info-restore-name'),
              contentPadding: EdgeInsets.zero,
              leading: const Icon(LucideIcons.undo2),
              title: const Text('Restore name'),
              onTap: () => ref
                  .read(channelNamesProvider.notifier)
                  .setName(channel.id, null),
            ),
          SwitchListTile.adaptive(
            key: const ValueKey('chat-info-mute'),
            contentPadding: EdgeInsets.zero,
            title: const Text('Mute notifications'),
            value: isMuted,
            onChanged: (mute) {
              final notifier = ref.read(channelMutesProvider.notifier);
              mute
                  ? notifier.muteChannel(channel.id)
                  : notifier.unmuteChannel(channel.id);
            },
          ),
          RoomGoalsSwitch(
            channel: displayed.value,
            canEdit: canAct,
            contentPadding: EdgeInsets.zero,
            onSaved: (enabled) => displayed.value = displayed.value.copyWith(
              goalsEnabled: enabled,
            ),
          ),
          const Divider(),
          for (final pubkey in memberPubkeys)
            _MemberTile(
              key: ValueKey('chat-info-member-$pubkey'),
              pubkey: pubkey,
              label: pubkey == me
                  ? '${names.labelFor(pubkey)} (you)'
                  : names.labelFor(pubkey),
            ),
          if (channel.isMember) ...[
            const Divider(),
            ListTile(
              key: const ValueKey('chat-info-leave'),
              contentPadding: EdgeInsets.zero,
              leading: Icon(LucideIcons.logOut, color: context.colors.error),
              title: Text(
                'Leave chat',
                style: TextStyle(color: context.colors.error),
              ),
              onTap: leave,
            ),
          ],
        ],
      ),
    );
  }
}

class _MemberTile extends ConsumerWidget {
  const _MemberTile({super.key, required this.pubkey, required this.label});

  final String pubkey;
  final String label;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final profile = ref.watch(
      userCacheProvider.select((cache) => cache[pubkey]),
    );
    return ListTile(
      contentPadding: EdgeInsets.zero,
      leading: ConversationPersonAvatar(
        pubkey: pubkey,
        profile: profile,
        isAgent:
            ref.watch(knownAgentPubkeysProvider).contains(pubkey) ||
            profile?.isAgent == true,
        size: 36,
      ),
      title: Text(label, maxLines: 1, overflow: TextOverflow.ellipsis),
      onTap: () => showUserProfileSheet(context, pubkey),
    );
  }
}

class _RenameDialog extends HookWidget {
  const _RenameDialog({required this.initial});

  final String initial;

  @override
  Widget build(BuildContext context) {
    final controller = useTextEditingController(text: initial);
    void save() => Navigator.of(context).pop(controller.text);
    return AlertDialog(
      title: const Text('Rename for me'),
      content: TextField(
        key: const ValueKey('chat-info-rename-field'),
        controller: controller,
        autofocus: true,
        maxLength: channelNameMaxLength,
        textInputAction: TextInputAction.done,
        onSubmitted: (_) => save(),
        decoration: const InputDecoration(
          helperText: 'Only you see this name.',
          counterText: '',
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          key: const ValueKey('chat-info-rename-save'),
          onPressed: save,
          child: const Text('Save'),
        ),
      ],
    );
  }
}
