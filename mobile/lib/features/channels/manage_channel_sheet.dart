import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/relay/relay.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import 'channel.dart';
import '../profile/avatar_upload_image.dart';
import '../profile/profile_avatar_crop_page.dart';
import 'channel_management_provider.dart';
import 'group_history_switch.dart';
import 'group_photo_field.dart';

class ManageChannelSheet extends HookConsumerWidget {
  const ManageChannelSheet({
    super.key,
    required this.channel,
    this.canEditDetails = true,
    this.onChannelUpdated,
  });

  final Channel channel;
  final bool canEditDetails;
  final ValueChanged<Channel>? onChannelUpdated;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final canvasAsync = ref.watch(channelCanvasProvider(channel.id));
    final nameController = useTextEditingController(text: channel.name);
    final descriptionController = useTextEditingController(
      text: channel.description,
    );
    useListenable(nameController);
    useListenable(descriptionController);
    final canvasController = useTextEditingController();
    final isEditingCanvas = useState(false);
    final isSavingCanvas = useState(false);
    final isSavingDetails = useState(false);
    final actionError = useState<String?>(null);
    final picture = useState<String?>(channel.picture);
    final historyAll = useState(channel.historyAll);
    final isSavingPhoto = useState(false);

    useEffect(() {
      final canvas = canvasAsync.asData?.value;
      if (!isEditingCanvas.value) {
        canvasController.text = canvas?.content ?? '';
      }
      return null;
    }, [canvasAsync.asData?.value.content, isEditingCanvas.value]);

    final canonicalName = nameController.text
        .trim()
        .replaceFirst(RegExp(r'^#+'), '')
        .trim();
    final description = descriptionController.text.trim();
    final nameDirty = canonicalName != channel.name.trim();
    final descriptionDirty = description != channel.description.trim();
    final canSaveDetails =
        canEditDetails &&
        canonicalName.isNotEmpty &&
        (nameDirty || descriptionDirty) &&
        !isSavingDetails.value;
    final canEditCanvas = channel.isMember && !channel.isArchived;

    Future<void> saveDetails() async {
      if (!canSaveDetails) return;
      isSavingDetails.value = true;
      actionError.value = null;
      try {
        await ref
            .read(channelActionsProvider)
            .updateChannel(
              channelId: channel.id,
              name: nameDirty ? canonicalName : null,
              description: descriptionDirty ? description : null,
            );
        final updated = channel.copyWith(
          name: canonicalName,
          description: description,
          picture: picture.value,
          historyAll: historyAll.value,
        );
        onChannelUpdated?.call(updated);
        if (context.mounted) Navigator.of(context).pop(false);
      } catch (error) {
        actionError.value = error.toString();
      } finally {
        isSavingDetails.value = false;
      }
    }

    Future<void> savePicture(String url) async {
      await ref
          .read(channelActionsProvider)
          .updateChannel(channelId: channel.id, picture: url);
      final next = url.isEmpty ? null : url;
      if (!context.mounted) return;
      picture.value = next;
      onChannelUpdated?.call(
        channel.copyWith(
          name: canonicalName.isEmpty ? channel.name : canonicalName,
          description: description,
          picture: next,
          historyAll: historyAll.value,
        ),
      );
    }

    Future<void> runPhotoChange(Future<void> Function() change) async {
      if (isSavingPhoto.value) return;
      isSavingPhoto.value = true;
      actionError.value = null;
      try {
        await change();
      } catch (_) {
        if (context.mounted) {
          actionError.value = context.l10n.channelGroupPhotoUpdateError;
        }
      } finally {
        if (context.mounted) isSavingPhoto.value = false;
      }
    }

    Future<void> changePhoto() => runPhotoChange(() async {
      final service = ref.read(mediaUploadServiceProvider);
      final picked = await service.pickGalleryImage();
      if (picked == null || !context.mounted) return;
      final prepared = await service.prepareImageBytes(picked);
      if (!context.mounted) return;
      // A GIF is uploaded as it is: cropping would drop its animation.
      if (isGifImage(prepared)) {
        final uploaded = await service.uploadBytes(
          prepared,
          mimeType: 'image/gif',
        );
        await savePicture(uploaded.url);
        return;
      }
      final cropped = await Navigator.of(context).push<Uint8List>(
        MaterialPageRoute(
          builder: (_) => ProfileAvatarCropPage(
            imageBytes: Future<Uint8List?>.value(prepared),
          ),
        ),
      );
      if (cropped == null || !context.mounted) return;
      final uploaded = await service.uploadBytes(
        cropped,
        mimeType: avatarUploadMimeType(cropped),
      );
      await savePicture(uploaded.url);
    });

    Future<void> removePhoto() => runPhotoChange(() => savePicture(''));

    Future<void> saveCanvas() async {
      if (isSavingCanvas.value) return;
      isSavingCanvas.value = true;
      actionError.value = null;
      try {
        await ref
            .read(channelActionsProvider)
            .setCanvas(
              channelId: channel.id,
              content: canvasController.text.trim(),
            );
        if (context.mounted) isEditingCanvas.value = false;
      } catch (error) {
        actionError.value = error.toString();
      } finally {
        isSavingCanvas.value = false;
      }
    }

    return Padding(
      padding: EdgeInsets.fromLTRB(
        Grid.gutter,
        0,
        Grid.gutter,
        MediaQuery.viewInsetsOf(context).bottom + Grid.xs,
      ),
      child: SafeArea(
        top: false,
        child: ListView(
          shrinkWrap: true,
          children: [
            if (canEditDetails) ...[
              GroupPhotoField(
                channel: channel,
                picture: picture.value,
                isSaving: isSavingPhoto.value,
                onChange: changePhoto,
                onRemove: removePhoto,
              ),
              const SizedBox(height: Grid.xs),
            ],
            _ManageChannelTextField(
              fieldKey: const ValueKey('manage-channel-name'),
              outlineKey: const ValueKey('manage-channel-name-outline'),
              controller: nameController,
              enabled: canEditDetails && !isSavingDetails.value,
              hintText: context.l10n.channelManageGroupName,
              textInputAction: TextInputAction.next,
            ),
            const SizedBox(height: Grid.xs),
            _ManageChannelTextField(
              fieldKey: const ValueKey('manage-channel-description'),
              outlineKey: const ValueKey('manage-channel-description-outline'),
              controller: descriptionController,
              enabled: canEditDetails && !isSavingDetails.value,
              hintText: context.l10n.channelCreateDescription,
              minLines: 2,
              maxLines: 4,
              textInputAction: TextInputAction.newline,
            ),
            if (canonicalName.isEmpty) ...[
              const SizedBox(height: Grid.half),
              Text(
                context.l10n.channelManageGroupNameRequired,
                style: context.textTheme.bodySmall?.copyWith(
                  color: context.colors.error,
                ),
              ),
            ],
            const SizedBox(height: Grid.xs),
            Align(
              alignment: AlignmentDirectional.centerEnd,
              child: FilledButton(
                key: const ValueKey('manage-channel-save-details'),
                onPressed: canSaveDetails ? saveDetails : null,
                child: Text(
                  isSavingDetails.value
                      ? context.l10n.commonSavingEllipsis
                      : context.l10n.channelManageSaveChanges,
                ),
              ),
            ),
            // Owners and admins only; DMs and group chats always hide
            // earlier messages.
            if (canEditDetails && !channel.isPersonChat) ...[
              const SizedBox(height: Grid.xs),
              GroupHistorySwitch(
                channel: channel.copyWith(historyAll: historyAll.value),
                contentPadding: const EdgeInsets.symmetric(
                  horizontal: Grid.half,
                ),
                onSaved: (next) {
                  historyAll.value = next;
                  onChannelUpdated?.call(
                    channel.copyWith(picture: picture.value, historyAll: next),
                  );
                },
              ),
            ],
            const SizedBox(height: Grid.sm),
            _ManageChannelSection(
              label: context.l10n.channelManageCanvas,
              child: canvasAsync.when(
                data: (canvas) {
                  if (isEditingCanvas.value) {
                    return Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        TextField(
                          key: const ValueKey('manage-channel-canvas'),
                          controller: canvasController,
                          maxLines: 8,
                          minLines: 6,
                          decoration: InputDecoration(
                            hintText: context.l10n.channelManageCanvasHint,
                          ),
                        ),
                        const SizedBox(height: Grid.xxs),
                        Row(
                          mainAxisAlignment: MainAxisAlignment.end,
                          children: [
                            TextButton(
                              onPressed: isSavingCanvas.value
                                  ? null
                                  : () {
                                      isEditingCanvas.value = false;
                                      canvasController.text =
                                          canvas.content ?? '';
                                    },
                              child: Text(context.l10n.commonCancel),
                            ),
                            const SizedBox(width: Grid.half),
                            FilledButton(
                              onPressed: isSavingCanvas.value
                                  ? null
                                  : saveCanvas,
                              child: Text(
                                isSavingCanvas.value
                                    ? context.l10n.commonSavingEllipsis
                                    : context.l10n.channelManageSaveCanvas,
                              ),
                            ),
                          ],
                        ),
                      ],
                    );
                  }

                  return Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Text(
                        canvas.content?.trim().isNotEmpty == true
                            ? canvas.content!
                            : context.l10n.channelManageCanvasEmpty,
                        style: context.textTheme.bodyMedium?.copyWith(
                          color: context.colors.onSurfaceVariant,
                        ),
                      ),
                      const SizedBox(height: Grid.xxs),
                      Align(
                        alignment: AlignmentDirectional.centerEnd,
                        child: FilledButton.tonal(
                          onPressed: canEditCanvas
                              ? () => isEditingCanvas.value = true
                              : null,
                          child: Text(
                            canvas.content?.trim().isNotEmpty == true
                                ? context.l10n.channelManageEditCanvas
                                : context.l10n.channelManageCreateCanvas,
                          ),
                        ),
                      ),
                    ],
                  );
                },
                loading: () => Center(
                  child: KuzzLoadingIndicator(
                    size: 40,
                    semanticLabel: context.l10n.channelManageCanvasLoading,
                  ),
                ),
                error: (error, _) => Text(
                  context.l10n.channelManageCanvasLoadError,
                  style: context.textTheme.bodySmall?.copyWith(
                    color: context.colors.error,
                  ),
                ),
              ),
            ),
            if (actionError.value case final error?) ...[
              const SizedBox(height: Grid.xxs),
              Text(
                error,
                style: context.textTheme.bodySmall?.copyWith(
                  color: context.colors.error,
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _ManageChannelTextField extends StatelessWidget {
  const _ManageChannelTextField({
    required this.fieldKey,
    required this.outlineKey,
    required this.controller,
    required this.enabled,
    required this.hintText,
    required this.textInputAction,
    this.minLines = 1,
    this.maxLines = 1,
  });

  final Key fieldKey;
  final Key outlineKey;
  final TextEditingController controller;
  final bool enabled;
  final String hintText;
  final TextInputAction textInputAction;
  final int minLines;
  final int maxLines;

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      key: outlineKey,
      decoration: BoxDecoration(
        border: Border.all(
          color: context.colors.outlineVariant.withValues(alpha: 0.8),
        ),
        borderRadius: BorderRadius.circular(Radii.card),
      ),
      child: Semantics(
        label: hintText,
        textField: true,
        child: TextField(
          key: fieldKey,
          controller: controller,
          enabled: enabled,
          minLines: minLines,
          maxLines: maxLines,
          style: context.textTheme.bodyLarge,
          decoration: InputDecoration(
            hintText: hintText,
            hintStyle: context.textTheme.bodyLarge?.copyWith(
              color: context.colors.onSurfaceVariant,
            ),
            border: InputBorder.none,
            enabledBorder: InputBorder.none,
            focusedBorder: InputBorder.none,
            isDense: true,
            contentPadding: const EdgeInsets.symmetric(
              horizontal: Grid.xs,
              vertical: Grid.twelve,
            ),
          ),
          textInputAction: textInputAction,
        ),
      ),
    );
  }
}

class _ManageChannelSection extends StatelessWidget {
  const _ManageChannelSection({required this.label, required this.child});

  final String label;
  final Widget child;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      Padding(
        padding: const EdgeInsetsDirectional.only(
          start: Grid.half,
          bottom: Grid.xxs,
        ),
        child: Text(
          label,
          style: context.textTheme.labelMedium?.copyWith(
            color: context.colors.onSurfaceVariant,
            fontWeight: FontWeight.w600,
          ),
        ),
      ),
      Material(
        color: context.colors.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(Radii.card),
        clipBehavior: Clip.antiAlias,
        child: Padding(padding: const EdgeInsets.all(Grid.xs), child: child),
      ),
    ],
  );
}
