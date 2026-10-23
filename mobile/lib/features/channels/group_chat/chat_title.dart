import 'dart:math';

import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../../shared/identity_names/identity_names.dart';
import '../../../shared/identity_names/identity_names_provider.dart';
import '../../../shared/l10n/l10n.dart';
import '../../../shared/relay/relay.dart';
import '../../../shared/utils/string_utils.dart';
import '../channel.dart';
import '../channel_names/channel_names_provider.dart';
import '../dm_channel_labels.dart';

/// Title of a group chat with nobody else in it.
String get emptyChatTitle => appL10n.chatEmptyTitle;

/// Members named in an unnamed group chat's title before "+N".
const chatTitleMemberLimit = 3;

/// "Ann, Bob, Cy +2": the first [chatTitleMemberLimit] names, then how many
/// more of [total] others (default: all [labels]). Empty when there are no
/// names.
String formatChatMemberNames(List<String> labels, {int? total}) {
  final visible = labels.take(chatTitleMemberLimit).join(', ');
  final count = total == null || total < labels.length ? labels.length : total;
  final hidden = count - min(labels.length, chatTitleMemberLimit);
  return hidden > 0 && visible.isNotEmpty ? '$visible +$hidden' : visible;
}

/// The one title rule for a room of people: my private name, then the
/// shared name, then the other members' names, then "Empty chat".
String resolveChatTitle({
  String? privateName,
  required String sharedName,
  required List<String> otherMemberLabels,
  int? otherMemberCount,
}) {
  final private = privateName?.trim();
  if (private != null && private.isNotEmpty) return private;
  final shared = sharedName.trim();
  if (shared.isNotEmpty) return shared;
  final members = formatChatMemberNames(
    otherMemberLabels,
    total: otherMemberCount,
  );
  return members.isEmpty ? emptyChatTitle : members;
}

/// Members other than [currentPubkey], lowercase, in the order given.
List<String> otherChatMembers(
  Iterable<String> pubkeys, {
  required String? currentPubkey,
}) {
  final me = currentPubkey?.toLowerCase();
  final seen = <String>{};
  return [
    for (final pubkey in pubkeys)
      if (pubkey.toLowerCase() != me && seen.add(pubkey.toLowerCase()))
        pubkey.toLowerCase(),
  ];
}

/// A room's title for lists, headers, search, Inbox and notifications:
/// group chats follow [resolveChatTitle] over [members] (default: the
/// channel's own), DMs their people, groups their name ("Group" if blank).
String channelTitle(
  Channel channel, {
  String? currentPubkey,
  IdentityNameSources? names,
  String? privateName,
  Iterable<String>? members,
}) {
  if (channel.isGroupChat) {
    final others = otherChatMembers(
      members ?? channel.participantPubkeys,
      currentPubkey: currentPubkey,
    );
    final scoped = names?.scope(others);
    return resolveChatTitle(
      privateName: privateName,
      sharedName: channel.name,
      otherMemberLabels: [
        for (final pubkey in others)
          scoped?.labelFor(pubkey) ?? shortPubkey(pubkey),
      ],
      // A partial member list (a chat list row's first 16) still counts
      // everyone in "+N".
      otherMemberCount: channel.memberCount > 0
          ? channel.memberCount - (channel.isMember ? 1 : 0)
          : null,
    );
  }
  final private = privateName?.trim();
  if (private != null && private.isNotEmpty) return private;
  if (channel.isDm) {
    return resolveDmChannelDisplayLabel(
      channel,
      currentPubkey: currentPubkey,
      names: names,
    );
  }
  final name = channel.name.trim();
  return name.isEmpty ? appL10n.chatGroupTitleFallback : name;
}

/// [channelTitle] with my private name and live profile names, loading the
/// profiles of the members it shows.
String watchChannelTitle(
  WidgetRef ref,
  Channel channel, {
  Iterable<String>? members,
}) {
  final currentPubkey = ref.watch(myPubkeyProvider);
  final privateName = ref.watch(channelPrivateNameProvider(channel.id));
  if (channel.isGroupChat) {
    final others = otherChatMembers(
      members ?? channel.participantPubkeys,
      currentPubkey: currentPubkey,
    );
    // Loads the named members' profiles; the title reads the same sources.
    watchIdentityNames(ref, others, shown: others.take(chatTitleMemberLimit));
  }
  return channelTitle(
    channel,
    currentPubkey: currentPubkey,
    names: ref.watch(identityNameSourcesProvider),
    privateName: privateName,
    members: members,
  );
}
