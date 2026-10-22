import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../../shared/mentions/agent_identity_provider.dart';
import '../../../shared/profile/user_cache_provider.dart';
import '../../../shared/relay/relay.dart';
import '../../../shared/theme/theme.dart';
import '../../../shared/widgets/conversation_row_parts.dart';
import 'chat_title.dart';

/// Where each member circle sits in a group chat photo, as fractions of the
/// avatar: (left, top, diameter). 1 = one circle; 2 = diagonal; 3 = one on
/// top and two below; 4 = 2x2.
List<({double left, double top, double size})> chatGridLayout(int count) {
  const half = 0.5;
  return switch (count) {
    <= 1 => const [(left: 0, top: 0, size: 1)],
    2 => const [(left: 0, top: 0, size: 0.6), (left: 0.4, top: 0.4, size: 0.6)],
    3 => const [
      (left: 0.25, top: 0, size: half),
      (left: 0, top: half, size: half),
      (left: half, top: half, size: half),
    ],
    _ => const [
      (left: 0, top: 0, size: half),
      (left: half, top: 0, size: half),
      (left: 0, top: half, size: half),
      (left: half, top: half, size: half),
    ],
  };
}

/// A group chat's photo, drawn from its members (never stored): up to four
/// of the other members' pictures in a grid; my own when I am alone. No
/// lock badge.
class ChatGridAvatar extends HookConsumerWidget {
  const ChatGridAvatar({
    super.key,
    required this.memberPubkeys,
    this.size = activityAvatarSize,
  });

  /// The chat's members (may include me), in member order.
  final List<String> memberPubkeys;

  /// Diameter of the whole avatar.
  final double size;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final me = ref.watch(myPubkeyProvider)?.toLowerCase();
    final others = otherChatMembers(memberPubkeys, currentPubkey: me);
    final people = (others.isEmpty && me != null ? [me] : others)
        .take(4)
        .toList();
    useEffect(() {
      final missing = [
        for (final pubkey in people)
          if (ref.read(userCacheProvider)[pubkey] == null) pubkey,
      ];
      if (missing.isNotEmpty) {
        unawaited(
          Future.microtask(
            () => ref.read(userCacheProvider.notifier).preload(missing),
          ),
        );
      }
      return null;
    }, [people.join(',')]);
    final layout = chatGridLayout(people.length);
    return SizedBox.square(
      key: const ValueKey('chat-grid-avatar'),
      dimension: size,
      child: Stack(
        children: [
          for (var i = 0; i < layout.length && i < people.length; i++)
            Positioned(
              left: layout[i].left * size,
              top: layout[i].top * size,
              width: layout[i].size * size,
              height: layout[i].size * size,
              child: _MemberCircle(
                key: ValueKey('chat-grid-avatar-${people[i]}'),
                pubkey: people[i],
                size: layout[i].size * size,
              ),
            ),
          if (people.isEmpty)
            ConversationLetterAvatar(
              seed: '',
              label: '',
              size: size,
              avatarKey: null,
            ),
        ],
      ),
    );
  }
}

class _MemberCircle extends ConsumerWidget {
  const _MemberCircle({super.key, required this.pubkey, required this.size});

  final String pubkey;
  final double size;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final profile = ref.watch(
      userCacheProvider.select((cache) => cache[pubkey]),
    );
    return ConversationPersonAvatar(
      pubkey: pubkey,
      profile: profile,
      isAgent:
          ref.watch(knownAgentPubkeysProvider).contains(pubkey) ||
          profile?.isAgent == true,
      size: size,
      fontSize: size / 2.6,
    );
  }
}
