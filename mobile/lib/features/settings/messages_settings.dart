import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/theme/grid.dart';
import '../../shared/widgets/app_list_card.dart';
import 'read_aloud_settings.dart';

/// Settings → Messages: how messages show on this device.
class MessagesSettings extends ConsumerWidget {
  const MessagesSettings({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return AppListCard(
      key: const ValueKey('settings-messages'),
      label: context.l10n.settingsMessagesSection,
      verticalPadding: Grid.twelve,
      children: readAloudSettingsRows(context, ref),
    );
  }
}
