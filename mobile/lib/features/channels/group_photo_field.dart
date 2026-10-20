import 'package:flutter/material.dart';

import '../../shared/theme/theme.dart';
import '../../shared/widgets/conversation_row_parts.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import 'channel.dart';

/// Diameter of the group photo at the top of the Manage group sheet.
const groupPhotoFieldSize = 88.0;

/// The group photo at the top of the Manage group sheet with Change photo
/// and, when a photo is set, Remove photo. Owners and admins only.
class GroupPhotoField extends StatelessWidget {
  const GroupPhotoField({
    super.key,
    required this.channel,
    required this.picture,
    required this.isSaving,
    required this.onChange,
    required this.onRemove,
  });

  final Channel channel;

  /// The photo shown now; null shows the letter.
  final String? picture;

  /// Whether a photo change is uploading or saving.
  final bool isSaving;
  final VoidCallback onChange;
  final VoidCallback onRemove;

  @override
  Widget build(BuildContext context) {
    final hasPicture = picture != null && picture!.trim().isNotEmpty;
    return Column(
      children: [
        SizedBox.square(
          dimension: groupPhotoFieldSize,
          child: Stack(
            alignment: Alignment.center,
            children: [
              ConversationLetterAvatar(
                seed: channel.id,
                label: channel.name,
                isPrivate: channel.isPrivate,
                size: groupPhotoFieldSize,
                avatarKey: const ValueKey('manage-channel-photo'),
                imageUrl: picture,
              ),
              if (isSaving)
                Container(
                  width: groupPhotoFieldSize,
                  height: groupPhotoFieldSize,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: Colors.black.withValues(alpha: 0.35),
                  ),
                  alignment: Alignment.center,
                  child: const KuzzLoadingIndicator(
                    size: 32,
                    semanticLabel: 'Saving group photo',
                  ),
                ),
            ],
          ),
        ),
        const SizedBox(height: Grid.xxs),
        Wrap(
          alignment: WrapAlignment.center,
          spacing: Grid.half,
          children: [
            TextButton(
              key: const ValueKey('manage-channel-change-photo'),
              onPressed: isSaving ? null : onChange,
              child: const Text('Change photo'),
            ),
            if (hasPicture)
              TextButton(
                key: const ValueKey('manage-channel-remove-photo'),
                onPressed: isSaving ? null : onRemove,
                style: TextButton.styleFrom(
                  foregroundColor: context.colors.error,
                ),
                child: const Text('Remove photo'),
              ),
          ],
        ),
      ],
    );
  }
}
