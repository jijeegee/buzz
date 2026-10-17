import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/mentions/agent_identity_provider.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/avatar_image.dart';

/// One Agents row: avatar, name, a short second line, and an
/// optional trailing action.
class AgentRow extends ConsumerWidget {
  const AgentRow({
    super.key,
    required this.pubkey,
    required this.name,
    required this.subtitle,
    required this.onTap,
    this.onLongPress,
    this.trailing,
  });

  final String pubkey;
  final String name;
  final String subtitle;
  final VoidCallback onTap;
  final VoidCallback? onLongPress;
  final Widget? trailing;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final profile = ref.watch(
      userCacheProvider.select((cache) => cache[pubkey]),
    );
    final isAgent =
        ref.watch(knownAgentPubkeysProvider).contains(pubkey) ||
        profile?.isAgent == true;
    final initial = name.trim().isNotEmpty
        ? String.fromCharCode(name.trim().runes.first).toUpperCase()
        : '?';
    return InkWell(
      onTap: onTap,
      onLongPress: onLongPress,
      child: Padding(
        padding: const EdgeInsets.symmetric(
          horizontal: Grid.gutter,
          vertical: Grid.xs,
        ),
        child: Row(
          children: [
            AvatarImage(
              imageUrl: profile?.avatarUrl,
              radius: activityAvatarSize / 2,
              backgroundColor: context.colors.primaryContainer,
              fallback: Text(
                initial,
                style: TextStyle(
                  fontSize: 14,
                  fontWeight: FontWeight.w600,
                  color: context.colors.onPrimaryContainer,
                ),
              ),
              isAgent: isAgent,
            ),
            const SizedBox(width: messageAvatarContentGap),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: activityUsernameTextStyle.copyWith(
                      color: context.colors.onSurface,
                    ),
                  ),
                  if (subtitle.isNotEmpty)
                    Text(
                      subtitle,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: activityTimestampTextStyle.copyWith(
                        color: context.colors.onSurfaceVariant,
                      ),
                    ),
                ],
              ),
            ),
            if (trailing != null) ...[
              const SizedBox(width: Grid.xxs),
              trailing!,
            ],
          ],
        ),
      ),
    );
  }
}

/// A small upper-case section title.
class AgentsSectionTitle extends StatelessWidget {
  const AgentsSectionTitle(this.title, {super.key});

  final String title;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(
      Grid.gutter,
      Grid.sm,
      Grid.gutter,
      Grid.half,
    ),
    child: Text(
      title.toUpperCase(),
      style: context.textTheme.labelSmall?.copyWith(
        color: context.colors.onSurfaceVariant,
        fontWeight: FontWeight.w700,
        letterSpacing: 0.5,
      ),
    ),
  );
}

/// The filter field at the top of Agents.
class AgentsFilterField extends StatelessWidget {
  const AgentsFilterField({
    super.key,
    required this.controller,
    required this.hint,
    required this.onChanged,
  });

  final TextEditingController controller;
  final String hint;
  final ValueChanged<String> onChanged;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(
      Grid.gutter,
      Grid.xxs,
      Grid.gutter,
      Grid.xxs,
    ),
    child: TextField(
      controller: controller,
      onChanged: onChanged,
      textInputAction: TextInputAction.search,
      decoration: InputDecoration(
        hintText: hint,
        isDense: true,
        prefixIcon: const Icon(Icons.search, size: 18),
        filled: true,
        fillColor: context.colors.surfaceContainerHighest.withValues(
          alpha: 0.6,
        ),
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(Grid.xs),
          borderSide: BorderSide.none,
        ),
      ),
    ),
  );
}
