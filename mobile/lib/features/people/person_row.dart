import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/profile/user_profile.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/avatar_image.dart';
import '../../shared/widgets/lucide_star_icon.dart';

/// What People surfaces call someone with no name yet — never a key.
String get unknownPersonLabel => appL10n.friendsUnknownPerson;

/// A person's name for People surfaces: display name, then handle, then
/// [unknownPersonLabel] — never a key.
String personName(UserProfile? profile, [PersonSummary? summary]) {
  final name = profile?.displayName?.trim();
  if (name != null && name.isNotEmpty) return name;
  final fromServer = summary?.displayName?.trim();
  if (fromServer != null && fromServer.isNotEmpty) return fromServer;
  final handle = profile?.nip05Handle?.trim();
  if (handle != null && handle.isNotEmpty) return handle;
  final username = summary?.username;
  if (username != null && username.isNotEmpty) return '@$username';
  return unknownPersonLabel;
}

/// What friends, block and report prompts call someone whose name is not
/// known (yet): never "Unknown" and never a fragment of their key.
String get unnamedPersonLabel => appL10n.friendsUnnamedPerson;

/// [name] as friends, block and report prompts show it mid-sentence:
/// [unnamedPersonLabel] when blank or "Unknown".
String personActionName(String? name) {
  final trimmed = name?.trim() ?? '';
  if (trimmed.isEmpty || trimmed == unknownPersonLabel) {
    return unnamedPersonLabel;
  }
  return trimmed;
}

/// [personActionName] at the start of a sentence.
String personActionSubject(String? name) {
  final who = personActionName(name);
  return who == unnamedPersonLabel ? appL10n.friendsUnnamedPersonSubject : who;
}

/// The cached profile of [pubkey], watched alone.
UserProfile? watchPersonProfile(WidgetRef ref, String pubkey) =>
    ref.watch(userCacheProvider.select((cache) => cache[pubkey]));

class PersonAvatar extends StatelessWidget {
  const PersonAvatar({
    super.key,
    required this.name,
    required this.avatarUrl,
    this.radius = 20,
  });

  final String name;
  final String? avatarUrl;
  final double radius;

  @override
  Widget build(BuildContext context) {
    final initial =
        name.isEmpty || name == 'Unknown' || name == unknownPersonLabel
        ? '?'
        : name.characters.first.toUpperCase();
    return ExcludeSemantics(
      child: AvatarImage(
        imageUrl: avatarUrl,
        radius: radius,
        backgroundColor: context.colors.primaryContainer,
        fallback: Text(
          initial,
          style:
              (radius > 30
                      ? context.textTheme.headlineMedium
                      : context.textTheme.labelLarge)
                  ?.copyWith(
                    color: context.colors.onPrimaryContainer,
                    fontWeight: FontWeight.w600,
                  ),
        ),
      ),
    );
  }
}

/// One person in a People list. The whole row is a single button.
class PersonRow extends ConsumerWidget {
  const PersonRow({
    super.key,
    required this.pubkey,
    required this.onTap,
    this.summary,
    this.isFavorite = false,
    this.subtitle,
  });

  final String pubkey;
  final PersonSummary? summary;
  final bool isFavorite;
  final String? subtitle;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final profile = watchPersonProfile(ref, pubkey);
    final name = personName(profile, summary);
    final avatarUrl = profile?.avatarUrl ?? summary?.avatarUrl;
    return Semantics(
      button: true,
      label: isFavorite ? context.l10n.friendsFavoriteRowLabel(name) : name,
      excludeSemantics: true,
      onTap: onTap,
      child: ListTile(
        onTap: onTap,
        contentPadding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
        leading: PersonAvatar(name: name, avatarUrl: avatarUrl),
        title: Row(
          children: [
            Flexible(
              child: Text(name, maxLines: 1, overflow: TextOverflow.ellipsis),
            ),
            if (isFavorite) ...[
              const SizedBox(width: Grid.half),
              LucideStarIcon(
                color: context.colors.primary,
                filled: true,
                size: 14,
              ),
            ],
          ],
        ),
        subtitle: subtitle == null
            ? null
            : Text(subtitle!, maxLines: 1, overflow: TextOverflow.ellipsis),
      ),
    );
  }
}
