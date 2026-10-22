import 'package:flutter/material.dart';

import '../../../shared/widgets/modal_presentation.dart';
import '../add_members_sheet.dart';
import '../channel.dart';
import '../channel_detail_page.dart';
import '../channel_management_provider.dart';
import 'chat_title.dart';

/// The other person in a 1:1 DM, or null for my own notes or a legacy DM
/// with more people.
String? dmOtherPerson(Channel channel, {required String? currentPubkey}) {
  if (!channel.isDm) return null;
  final others = otherChatMembers(
    channel.participantPubkeys,
    currentPubkey: currentPubkey,
  );
  return others.length == 1 ? others.single : null;
}

/// A 1:1's "Add people": picks people, then starts a new chat with the
/// other person and them (the 1:1 itself is unchanged) and opens it.
Future<void> showAddPeopleToDm({
  required BuildContext context,
  required ChannelActions actions,
  required String otherPubkey,
  required Channel dm,
}) async {
  final navigator = Navigator.of(context, rootNavigator: true);
  final messenger = ScaffoldMessenger.maybeOf(context);
  Channel? created;
  await showKuzzModalBottomSheet<bool>(
    context: context,
    title: 'Add people',
    isScrollControlled: true,
    showDragHandle: true,
    builder: (_) => AddChannelMembersSheet(
      channelId: dm.id,
      existingPubkeys: {...dm.participantPubkeys},
      chat: true,
      startChat: (pubkeys) async {
        final result = await actions.createGroupChat(
          pubkeys: [otherPubkey, ...pubkeys],
        );
        created = result.channel;
        if (result.notice case final notice?) {
          messenger?.showSnackBar(SnackBar(content: Text(notice)));
        }
      },
    ),
  );
  final channel = created;
  if (channel == null || !navigator.mounted) return;
  await navigator.push(
    MaterialPageRoute<void>(
      builder: (_) => ChannelDetailPage(channel: channel),
    ),
  );
}
