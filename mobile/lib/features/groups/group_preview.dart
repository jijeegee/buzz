import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/deeplink/group_links.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/widgets/modal_presentation.dart';
import '../channels/channel.dart';
import '../channels/channel_detail_page.dart';
import '../channels/channel_management_provider.dart';
import '../channels/channels_provider.dart';
import '../channels/group_avatar.dart';
import 'groups_api.dart';

/// "1 member" / "N members".
String memberCountLabel(int count) => appL10n.groupsMemberCount(count);

/// The group [channelId] from the refreshed channel list, or [fallback]
/// marked as joined while the list catches up.
Future<Channel> joinedChannel(
  WidgetRef ref,
  String channelId,
  GroupSummary fallback,
) async {
  try {
    await ref.read(channelsProvider.notifier).refresh();
    final channels = await ref.read(channelsProvider.future);
    for (final channel in channels) {
      if (channel.id == channelId) return channel;
    }
  } catch (_) {
    // Open what we know; the page loads the rest.
  }
  return fallback.toChannel().copyWith(isMember: true);
}

/// Opens a group's page (read-only with Join group when not a member).
Future<void> openGroupPage(BuildContext context, Channel channel) =>
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => ChannelDetailPage(channel: channel),
      ),
    );

/// Looks up what a group link points to.
Future<GroupSummary> lookupGroupLink(GroupsApi api, GroupLinkTarget target) =>
    switch (target) {
      GroupInviteCodeTarget(:final code) => api.previewInvite(code),
      GroupHandleTarget(:final handle) => api.byHandle(handle),
    };

/// Joins through a link: the invite code route, or the ordinary public join
/// for a public address. Returns the group id.
Future<String> joinGroupByLink(
  WidgetRef ref,
  GroupLinkTarget target,
  GroupSummary group,
) async {
  switch (target) {
    case GroupInviteCodeTarget(:final code):
      return ref.read(groupsApiProvider).joinInvite(code);
    case GroupHandleTarget():
      await ref.read(channelActionsProvider).joinChannel(group.id);
      return group.id;
  }
}

/// User-facing text for a failed join.
String joinErrorMessage(Object error) {
  if (error is GroupsApiException) {
    if (error.isNotFound) return appL10n.groupsLinkNotFound;
    if (error.statusCode == 403) return appL10n.groupsCantJoin;
  }
  return appL10n.groupsJoinError;
}

/// Full-width Join group bar that replaces the composer while previewing a
/// public group, with its description above the button.
class GroupJoinBar extends HookConsumerWidget {
  const GroupJoinBar({super.key, required this.channel});

  final Channel channel;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final joining = useState(false);

    Future<void> join() async {
      if (joining.value) return;
      joining.value = true;
      try {
        await ref.read(channelActionsProvider).joinChannel(channel.id);
      } catch (_) {
        if (context.mounted) {
          ScaffoldMessenger.maybeOf(context)?.showSnackBar(
            SnackBar(content: Text(context.l10n.groupsJoinError)),
          );
        }
      } finally {
        if (context.mounted) joining.value = false;
      }
    }

    final description = channel.description.trim();
    return Container(
      key: const ValueKey('group-join-bar'),
      width: double.infinity,
      padding: EdgeInsets.fromLTRB(
        Grid.gutter,
        Grid.xs,
        Grid.gutter,
        MediaQuery.viewPaddingOf(context).bottom + Grid.xs,
      ),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: context.colors.outlineVariant)),
        color: context.colors.surface,
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (description.isNotEmpty) ...[
            Text(
              description,
              key: const ValueKey('group-join-bar-description'),
              maxLines: 3,
              overflow: TextOverflow.ellipsis,
              textAlign: TextAlign.center,
              style: context.textTheme.bodyMedium?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
            const SizedBox(height: Grid.xs),
          ],
          FilledButton(
            key: const ValueKey('group-join-bar-join'),
            onPressed: joining.value ? null : () => unawaited(join()),
            child: Text(
              joining.value
                  ? context.l10n.groupsJoining
                  : context.l10n.groupsJoinGroup,
            ),
          ),
        ],
      ),
    );
  }
}

/// The preview of a group found through a link: avatar, name, N members,
/// and (public groups only) the description, then Join group.
class GroupPreviewCard extends StatelessWidget {
  const GroupPreviewCard({
    super.key,
    required this.group,
    required this.onJoin,
    this.joining = false,
  });

  final GroupSummary group;
  final VoidCallback? onJoin;
  final bool joining;

  @override
  Widget build(BuildContext context) {
    final description = group.isPrivate ? '' : group.description.trim();
    return Container(
      key: ValueKey('group-preview-${group.id}'),
      padding: const EdgeInsets.all(Grid.sm),
      decoration: BoxDecoration(
        color: context.colors.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(Radii.card),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          GroupAvatar(channel: group.toChannel(), size: 64),
          const SizedBox(height: Grid.xs),
          Text(
            group.name,
            textAlign: TextAlign.center,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: context.textTheme.titleMedium?.copyWith(
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: Grid.quarter),
          Text(
            memberCountLabel(group.memberCount),
            style: context.textTheme.bodySmall?.copyWith(
              color: context.colors.onSurfaceVariant,
            ),
          ),
          if (description.isNotEmpty) ...[
            const SizedBox(height: Grid.xs),
            Text(
              description,
              textAlign: TextAlign.center,
              maxLines: 4,
              overflow: TextOverflow.ellipsis,
              style: context.textTheme.bodyMedium,
            ),
          ],
          const SizedBox(height: Grid.sm),
          SizedBox(
            width: double.infinity,
            child: FilledButton(
              key: const ValueKey('group-preview-join'),
              onPressed: joining ? null : onJoin,
              child: Text(
                group.isMember
                    ? context.l10n.groupsOpen
                    : joining
                    ? context.l10n.groupsJoining
                    : context.l10n.groupsJoinGroup,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Opens Join with link, optionally already looking up [target] (from a
/// deep link), then the group joined there.
Future<void> showJoinWithLinkSheet(
  BuildContext context, {
  GroupLinkTarget? target,
}) async {
  final mediaQuery = MediaQuery.of(context);
  final channel = await showKuzzModalBottomSheet<Channel>(
    context: context,
    title: context.l10n.groupsJoinWithLinkTitle,
    isScrollControlled: true,
    showDragHandle: true,
    constraints: BoxConstraints(
      maxHeight: mediaQuery.size.height - mediaQuery.viewPadding.top - Grid.sm,
    ),
    builder: (_) => JoinWithLinkSheet(initialTarget: target),
  );
  if (channel != null && context.mounted) {
    await openGroupPage(context, channel);
  }
}

/// Paste box for an invite link, page link, app link, code or `@address`,
/// with the group's preview card once found. Pops with the joined [Channel].
class JoinWithLinkSheet extends HookConsumerWidget {
  const JoinWithLinkSheet({super.key, this.initialTarget});

  final GroupLinkTarget? initialTarget;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final controller = useTextEditingController(
      text: switch (initialTarget) {
        GroupInviteCodeTarget(:final code) => code,
        GroupHandleTarget(:final handle) => '@$handle',
        null => '',
      },
    );
    useListenable(controller);
    final looking = useState(false);
    final joining = useState(false);
    final error = useState<String?>(null);
    final found = useState<(GroupLinkTarget, GroupSummary)?>(null);
    final lookupSerial = useRef(0);
    final l10n = context.l10n;

    Future<void> lookUp() async {
      final target = parseGroupLinkInput(controller.text);
      found.value = null;
      if (target == null) {
        error.value = l10n.groupsNotAGroupLink;
        return;
      }
      final serial = ++lookupSerial.value;
      looking.value = true;
      error.value = null;
      try {
        final group = await lookupGroupLink(
          ref.read(groupsApiProvider),
          target,
        );
        if (!context.mounted || serial != lookupSerial.value) return;
        found.value = (target, group);
      } catch (e) {
        if (!context.mounted || serial != lookupSerial.value) return;
        error.value = e is GroupsApiException && e.isNotFound
            ? l10n.groupsLinkNotFound
            : l10n.groupsLookupError;
      } finally {
        if (context.mounted && serial == lookupSerial.value) {
          looking.value = false;
        }
      }
    }

    Future<void> join() async {
      final current = found.value;
      if (current == null || joining.value) return;
      final (target, group) = current;
      final navigator = Navigator.of(context);
      joining.value = true;
      error.value = null;
      try {
        final channelId = group.isMember
            ? group.id
            : await joinGroupByLink(ref, target, group);
        final channel = await joinedChannel(ref, channelId, group);
        if (context.mounted) navigator.pop(channel);
      } catch (e) {
        if (context.mounted) error.value = joinErrorMessage(e);
      } finally {
        if (context.mounted) joining.value = false;
      }
    }

    useEffect(() {
      if (initialTarget != null) unawaited(Future<void>.microtask(lookUp));
      return null;
    }, const []);

    final hasText = controller.text.trim().isNotEmpty;
    return SafeArea(
      top: false,
      child: Padding(
        padding: EdgeInsets.fromLTRB(
          Grid.gutter,
          0,
          Grid.gutter,
          MediaQuery.viewInsetsOf(context).bottom + Grid.xs,
        ),
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              DecoratedBox(
                decoration: BoxDecoration(
                  border: Border.all(
                    color: context.colors.outlineVariant.withValues(alpha: 0.8),
                  ),
                  borderRadius: BorderRadius.circular(Radii.card),
                ),
                child: TextField(
                  key: const ValueKey('join-with-link-input'),
                  controller: controller,
                  autofocus: initialTarget == null,
                  autocorrect: false,
                  enableSuggestions: false,
                  keyboardType: TextInputType.url,
                  textInputAction: TextInputAction.go,
                  onChanged: (_) {
                    if (found.value != null || error.value != null) {
                      found.value = null;
                      error.value = null;
                    }
                  },
                  onSubmitted: (_) => unawaited(lookUp()),
                  decoration: InputDecoration(
                    hintText: l10n.groupsLinkHint,
                    prefixIcon: Icon(
                      LucideIcons.link,
                      size: 18,
                      color: context.colors.onSurfaceVariant,
                    ),
                    border: InputBorder.none,
                    enabledBorder: InputBorder.none,
                    focusedBorder: InputBorder.none,
                    contentPadding: const EdgeInsets.symmetric(
                      horizontal: Grid.twelve,
                      vertical: Grid.xs,
                    ),
                  ),
                ),
              ),
              const SizedBox(height: Grid.xs),
              if (found.value == null)
                FilledButton(
                  key: const ValueKey('join-with-link-continue'),
                  onPressed: hasText && !looking.value
                      ? () => unawaited(lookUp())
                      : null,
                  child: looking.value
                      ? const KuzzLoadingIndicator(size: 20)
                      : Text(l10n.groupsContinue),
                ),
              if (found.value case (_, final group))
                GroupPreviewCard(
                  group: group,
                  joining: joining.value,
                  onJoin: () => unawaited(join()),
                ),
              if (error.value case final message?) ...[
                const SizedBox(height: Grid.xs),
                Text(
                  message,
                  key: const ValueKey('join-with-link-error'),
                  textAlign: TextAlign.center,
                  style: context.textTheme.bodySmall?.copyWith(
                    color: context.colors.error,
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
