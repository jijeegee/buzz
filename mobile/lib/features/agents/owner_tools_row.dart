import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../settings/account_actions.dart';

/// Owner tools (AI tools S1): the relay's per-agent switch, set only by the
/// owner from a proven device. Mirrors desktop's `auth/tool_levels.rs`.

/// One agent's owner tools on the active account; `null` when unknown (no
/// account, not registered). No retry: a failed read shows the row locked
/// until the page is opened again.
final agentToolLevelProvider = FutureProvider.autoDispose
    .family<AgentToolLevel?, String>((ref, agentPubkey) async {
      final origin = ref.watch(accountOriginProvider);
      if (origin == null) return null;
      final key = agentPubkey.toLowerCase();
      final levels = await ref.watch(accountApiProvider(origin)).toolLevels([
        key,
      ]);
      return levels[key];
    }, retry: (_, _) => null);

/// The call a phone access save makes first.
@immutable
class ToolLevelWrite {
  const ToolLevelWrite({required this.level, required this.mustSucceed});

  final String level;

  /// The save fails when this call fails (the level is on).
  final bool mustSucceed;

  @override
  bool operator ==(Object other) =>
      other is ToolLevelWrite &&
      other.level == level &&
      other.mustSucceed == mustSucceed;

  @override
  int get hashCode => Object.hash(level, mustSucceed);
}

/// Before a save to [respondClass]: the level kept (off for anyone), or
/// nothing when unknown or never set. Anyone always sends off, even when the
/// level couldn't be read, so full is never left behind. Pure.
ToolLevelWrite? toolLevelSaveWrite(
  AgentToolLevel? current, {
  required String respondClass,
}) {
  if (respondClass == 'anyone') {
    return const ToolLevelWrite(level: 'limited', mustSucceed: false);
  }
  final level = current?.level;
  if (level == null) return null;
  final effective = respondClass == 'anyone' ? 'limited' : level;
  return ToolLevelWrite(level: effective, mustSucceed: effective == 'full');
}

/// Send [agentPubkey]'s owner tools before an access save to
/// [respondClass]. Throws only when the level is on and the call failed;
/// a stale class (the relay hasn't seen the new class yet) is logged, since
/// the relay's 30177 sync moves it.
Future<void> writeToolLevelBeforeSave(
  AccountApi api,
  String agentPubkey, {
  required AgentToolLevel? current,
  required String respondClass,
}) async {
  final write = toolLevelSaveWrite(current, respondClass: respondClass);
  if (write == null) return;
  try {
    await api.setToolLevel(
      agentPubkey,
      level: write.level,
      respondClass: respondClass,
    );
  } catch (failure) {
    final stale =
        failure is AccountApiException && failure.code == 'respond_class_stale';
    if (write.mustSucceed && !stale) rethrow;
    debugPrint('[OwnerTools] level not sent before save: $failure');
  }
}

/// Owner tools off before a disconnect narrows the agent. Logged, never in
/// the way of the disconnect.
Future<void> lowerToolLevelBeforeDisconnect(
  AccountApi api,
  String agentPubkey,
) async {
  try {
    await api.setToolLevel(
      agentPubkey,
      level: 'limited',
      respondClass: 'owner-only',
    );
  } on AccountApiException catch (failure) {
    if (failure.statusCode == 404) return;
    debugPrint('[OwnerTools] not turned off before disconnect: $failure');
  } catch (failure) {
    debugPrint('[OwnerTools] not turned off before disconnect: $failure');
  }
}

/// A refused or failed owner tools call, in words.
String toolLevelErrorMessage(BuildContext context, Object failure) {
  final code = failure is AccountApiException ? failure.code : null;
  return switch (code) {
    'respond_class_stale' => context.l10n.ownerToolsChangeOnComputer,
    'full_anyone_refused' => context.l10n.ownerToolsAnyoneDisabled,
    _ => context.l10n.ownerToolsSaveFailed,
  };
}

/// "Owner tools: On/Off" for one agent. Off and locked while anyone can talk
/// to it; turning it on for an allowlist asks first. [respondClass]: who the
/// agent answers when the page knows it (a paired agent); otherwise the
/// relay's stored class is used.
class OwnerToolsRow extends HookConsumerWidget {
  const OwnerToolsRow({
    super.key,
    required this.agentPubkey,
    this.respondClass,
  });

  final String agentPubkey;
  final String? respondClass;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final saving = useState(false);
    final error = useState<String?>(null);
    final origin = ref.watch(accountOriginProvider);
    if (origin == null) return const SizedBox.shrink();
    final loaded = ref.watch(agentToolLevelProvider(agentPubkey));
    // Nothing while loading, or for a key the relay doesn't list as this
    // owner's agent; a failed read shows the row off and locked.
    if ((!loaded.hasValue && !loaded.hasError) ||
        (loaded.hasValue && loaded.value == null)) {
      return const SizedBox.shrink();
    }
    final current = loaded.asData?.value;
    final unknown = current == null;
    final klass = respondClass ?? current?.respondClass;
    final anyone = klass == 'anyone';
    final on = !anyone && (current?.on ?? false);

    Future<void> change(bool value) async {
      final l10n = context.l10n;
      if (value && klass == 'allowlist') {
        final ok = await confirmDestructive(
          context,
          title: l10n.ownerToolsConfirmTitle,
          body: l10n.ownerToolsConfirmBody,
          action: l10n.ownerToolsConfirmAction,
        );
        if (!ok || !context.mounted) return;
      }
      saving.value = true;
      error.value = null;
      try {
        await ref
            .read(accountApiProvider(origin))
            .setToolLevel(
              agentPubkey,
              level: value ? 'full' : 'limited',
              // A phone can't know an in-app agent's class it was never
              // told: the relay then refuses (respond_class_stale).
              respondClass: klass ?? 'owner-only',
            );
        ref.invalidate(agentToolLevelProvider(agentPubkey));
      } catch (failure) {
        if (context.mounted) {
          error.value = toolLevelErrorMessage(context, failure);
        }
      } finally {
        if (context.mounted) saving.value = false;
      }
    }

    final hint = anyone
        ? context.l10n.ownerToolsAnyoneDisabled
        : unknown
        ? context.l10n.ownerToolsUnknown
        : on && klass == 'allowlist'
        ? context.l10n.ownerToolsAllowlistWarning
        : context.l10n.ownerToolsHint;
    return Column(
      key: const Key('owner-tools-row'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        SwitchListTile(
          key: const Key('owner-tools-switch'),
          contentPadding: EdgeInsets.zero,
          title: Text(
            '${context.l10n.ownerToolsLabel}: '
            '${on ? context.l10n.ownerToolsOn : context.l10n.ownerToolsOff}',
          ),
          subtitle: Text(
            hint,
            style: on && klass == 'allowlist'
                ? context.textTheme.bodySmall?.copyWith(
                    color: context.colors.error,
                  )
                : null,
          ),
          value: on,
          onChanged: anyone || unknown || saving.value
              ? null
              : (value) => unawaited(change(value)),
        ),
        if (error.value case final message?)
          Padding(
            padding: const EdgeInsets.only(bottom: Grid.xxs),
            child: Text(
              message,
              key: const Key('owner-tools-error'),
              style: context.textTheme.bodyMedium?.copyWith(
                color: context.colors.error,
              ),
            ),
          ),
      ],
    );
  }
}
