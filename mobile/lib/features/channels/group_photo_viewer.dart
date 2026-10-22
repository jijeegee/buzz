import 'package:flutter/material.dart';

import 'media_viewer_page.dart';

/// Makes a large group photo open full size in the image viewer.
///
/// Small avatars draw the relay's thumbnail; tapping shows the original
/// upload. With no [picture], [child] is returned as it is.
class GroupPhotoViewerTap extends StatelessWidget {
  const GroupPhotoViewerTap({
    super.key,
    required this.picture,
    required this.child,
  });

  /// The group photo URL; null or blank disables the tap.
  final String? picture;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final url = picture?.trim();
    if (url == null || url.isEmpty) return child;
    void open() => openImageViewer(
      context,
      imageUrl: url,
      heroTag: Object(),
      semanticLabel: 'Group photo',
    );
    return Semantics(
      button: true,
      label: 'View group photo',
      onTap: open,
      excludeSemantics: true,
      child: GestureDetector(
        key: const ValueKey('group-photo-open'),
        behavior: HitTestBehavior.opaque,
        onTap: open,
        child: child,
      ),
    );
  }
}
