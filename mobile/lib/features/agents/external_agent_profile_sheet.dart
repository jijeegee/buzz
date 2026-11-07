import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/relay/relay.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/avatar_image.dart';
import '../../shared/widgets/modal_presentation.dart';
import '../profile/avatar_upload_image.dart';
import '../profile/profile_avatar_crop_page.dart';

/// Longest description, matching the relay and the desktop dialog.
const externalAgentAboutMaxLength = 280;

/// Edit a paired agent's name, photo and description. The relay keeps them,
/// so this works from any device while the agent's computer is off.
Future<void> showExternalAgentProfileSheet(
  BuildContext context, {
  required String agentPubkey,
  required String name,
}) => showKuzzModalBottomSheet<void>(
  context: context,
  title: context.l10n.agentProfileEdit,
  isScrollControlled: true,
  useSafeArea: true,
  builder: (_) => _ProfileLoader(agentPubkey: agentPubkey, name: name),
);

/// Saving replaces all three fields, so the form opens only on the relay's
/// current answer: a stale or missing cache entry would clear them.
class _ProfileLoader extends HookConsumerWidget {
  const _ProfileLoader({required this.agentPubkey, required this.name});

  final String agentPubkey;
  final String name;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final read = useMemoized(
      () => ref.read(userCacheProvider.notifier).refresh([agentPubkey]),
      [agentPubkey],
    );
    final answer = useFuture(read);
    if (answer.connectionState != ConnectionState.done) {
      return const Padding(
        padding: EdgeInsets.all(Grid.md),
        child: Center(child: CircularProgressIndicator()),
      );
    }
    if (answer.data != true) {
      return Padding(
        padding: const EdgeInsets.all(Grid.xs),
        child: Text(
          context.l10n.agentAccessUnavailable,
          key: const Key('agent-profile-unavailable'),
          textAlign: TextAlign.center,
        ),
      );
    }
    return _ProfileForm(agentPubkey: agentPubkey, name: name);
  }
}

class _ProfileForm extends HookConsumerWidget {
  const _ProfileForm({required this.agentPubkey, required this.name});

  final String agentPubkey;
  final String name;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final key = agentPubkey.toLowerCase();
    final profile = ref.read(userCacheProvider)[key];
    final nameField = useTextEditingController(
      text: profile?.displayName?.trim().isNotEmpty == true
          ? profile!.displayName!.trim()
          : name,
    );
    // What the agent wrote may be longer than the relay takes.
    final aboutField = useTextEditingController(
      text: String.fromCharCodes(
        (profile?.about ?? '').runes.take(externalAgentAboutMaxLength),
      ),
    );
    final picture = useState<String?>(profile?.avatarUrl);
    final busy = useState(false);
    final error = useState<String?>(null);
    useListenable(nameField);

    Future<void> run(Future<void> Function() change) async {
      if (busy.value) return;
      busy.value = true;
      error.value = null;
      try {
        await change();
      } catch (_) {
        if (context.mounted) error.value = context.l10n.agentAccessSaveFailed;
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    Future<void> changePhoto() => run(() async {
      final service = ref.read(mediaUploadServiceProvider);
      final picked = await service.pickGalleryImage();
      if (picked == null || !context.mounted) return;
      final prepared = await service.prepareImageBytes(picked);
      if (!context.mounted) return;
      // A GIF is uploaded as it is: cropping would drop its animation.
      final bytes = isGifImage(prepared)
          ? prepared
          : await Navigator.of(context).push<Uint8List>(
              MaterialPageRoute(
                builder: (_) => ProfileAvatarCropPage(
                  imageBytes: Future<Uint8List?>.value(prepared),
                ),
              ),
            );
      if (bytes == null) return;
      final uploaded = await service.uploadBytes(
        bytes,
        mimeType: avatarUploadMimeType(bytes),
      );
      if (context.mounted) picture.value = uploaded.url;
    });

    Future<void> save() => run(() async {
      final sessionKey = ref.read(accountSessionProvider);
      if (sessionKey == null) {
        error.value = context.l10n.agentAccessUnavailable;
        return;
      }
      final navigator = Navigator.of(context);
      final about = aboutField.text.trim();
      await ref
          .read(accountApiProvider(sessionKey))
          .setExternalAgentProfile(
            key,
            displayName: nameField.text.trim(),
            avatarUrl: picture.value,
            about: about.isEmpty ? null : about,
          );
      unawaited(ref.read(userCacheProvider.notifier).refresh([key]));
      navigator.pop();
    });

    final hasPicture = picture.value?.trim().isNotEmpty ?? false;
    return SingleChildScrollView(
      padding: EdgeInsets.fromLTRB(
        Grid.xs,
        0,
        Grid.xs,
        Grid.xs + MediaQuery.viewInsetsOf(context).bottom,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          Center(
            child: AvatarImage(
              imageUrl: picture.value,
              radius: 48,
              isAgent: true,
              fallback: Text(
                name.characters.take(1).toString().toUpperCase(),
                style: context.textTheme.headlineMedium,
              ),
            ),
          ),
          const SizedBox(height: Grid.xxs),
          Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              TextButton(
                key: const Key('agent-profile-photo'),
                onPressed: busy.value ? null : changePhoto,
                child: Text(context.l10n.agentProfileChangePhoto),
              ),
              if (hasPicture)
                TextButton(
                  key: const Key('agent-profile-photo-remove'),
                  onPressed: busy.value ? null : () => picture.value = null,
                  child: Text(context.l10n.agentProfileRemovePhoto),
                ),
            ],
          ),
          const SizedBox(height: Grid.xxs),
          TextField(
            key: const Key('agent-profile-name'),
            controller: nameField,
            enabled: !busy.value,
            inputFormatters: [LengthLimitingTextInputFormatter(64)],
            decoration: InputDecoration(
              labelText: context.l10n.agentProfileName,
            ),
          ),
          const SizedBox(height: Grid.xs),
          TextField(
            key: const Key('agent-profile-about'),
            controller: aboutField,
            enabled: !busy.value,
            minLines: 2,
            maxLines: 4,
            maxLength: externalAgentAboutMaxLength,
            decoration: InputDecoration(
              labelText: context.l10n.agentProfileDescription,
            ),
          ),
          if (error.value != null) ...[
            const SizedBox(height: Grid.xxs),
            Text(
              error.value!,
              key: const Key('agent-profile-error'),
              style: context.textTheme.bodyMedium?.copyWith(
                color: context.colors.error,
              ),
            ),
          ],
          const SizedBox(height: Grid.xs),
          FilledButton(
            key: const Key('agent-profile-save'),
            onPressed: busy.value || nameField.text.trim().isEmpty
                ? null
                : save,
            child: Text(context.l10n.commonSave),
          ),
        ],
      ),
    );
  }
}
