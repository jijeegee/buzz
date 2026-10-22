import * as React from "react";

import { GroupLetterAvatar } from "@/features/home/ui/ConversationLetterAvatar";
import { useAvatarUpload } from "@/features/profile/useAvatarUpload";
import { rewriteRelayUrl } from "@/shared/lib/mediaUrl";
import { Button } from "@/shared/ui/button";
import { SimpleImageLightbox } from "@/shared/ui/SimpleImageLightbox";
import { Spinner } from "@/shared/ui/spinner";
import { useT } from "@/shared/i18n";

/**
 * Group photo on top of the group edit dialog: the group's avatar (photo, or
 * the letter on color) with Change photo and Remove photo. Uploads go
 * through the same path and image types as the profile photo; the parent
 * sends the resulting URL with Save changes. Clicking the photo opens the
 * original full-size; the avatar itself shows the relay's small thumbnail.
 */
export function GroupPhotoField({
  channelId,
  disabled = false,
  isPrivate,
  label,
  onPictureChange,
  onUploadingChange,
  picture,
}: {
  channelId: string;
  disabled?: boolean;
  isPrivate: boolean;
  label: string;
  /** New photo URL, or "" when removed. */
  onPictureChange: (picture: string) => void;
  onUploadingChange?: (isUploading: boolean) => void;
  picture: string;
}) {
  const t = useT();
  const { errorMessage, handleFileChange, inputRef, isUploading, openPicker } =
    useAvatarUpload({
      fallbackErrorMessage: t("channels.photo.uploadFailed"),
      onUploadSuccess: onPictureChange,
    });

  React.useEffect(() => {
    onUploadingChange?.(isUploading);
  }, [isUploading, onUploadingChange]);

  const busy = disabled || isUploading;
  const [isViewerOpen, setIsViewerOpen] = React.useState(false);
  const fullSizeUrl = picture.trim();
  const avatar = (
    <GroupLetterAvatar
      channelId={channelId}
      isPrivate={isPrivate}
      label={label}
      picture={picture || null}
      size="xl"
      testId="group-photo-avatar"
    />
  );

  return (
    <div className="flex flex-col items-center gap-3" data-testid="group-photo">
      <div className="relative">
        {fullSizeUrl ? (
          <button
            aria-label={t("channels.photo.view")}
            className="block cursor-zoom-in rounded-full focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
            data-testid="group-photo-view"
            onClick={() => setIsViewerOpen(true)}
            type="button"
          >
            {avatar}
          </button>
        ) : (
          avatar
        )}
        {isUploading ? (
          <span
            aria-label={t("channels.photo.uploading")}
            className="absolute inset-0 flex items-center justify-center rounded-full bg-black/35 text-white"
            role="status"
          >
            <Spinner aria-hidden="true" className="border-2" size={20} />
          </span>
        ) : null}
      </div>
      <div className="flex items-center gap-2">
        <Button
          data-testid="group-photo-change"
          disabled={busy}
          onClick={openPicker}
          size="sm"
          type="button"
          variant="outline"
        >
          {t("channels.photo.change")}
        </Button>
        {picture ? (
          <Button
            data-testid="group-photo-remove"
            disabled={busy}
            onClick={() => onPictureChange("")}
            size="sm"
            type="button"
            variant="ghost"
          >
            {t("channels.photo.remove")}
          </Button>
        ) : null}
      </div>
      {errorMessage ? (
        <p className="text-sm text-destructive" data-testid="group-photo-error">
          {errorMessage}
        </p>
      ) : null}
      {fullSizeUrl ? (
        <SimpleImageLightbox
          alt={
            label.trim()
              ? t("channels.photo.altNamed", { name: label.trim() })
              : t("channels.photo.alt")
          }
          onOpenChange={setIsViewerOpen}
          open={isViewerOpen}
          src={rewriteRelayUrl(fullSizeUrl)}
        />
      ) : null}
      <input
        accept="image/png,image/jpeg,image/gif,image/webp"
        className="hidden"
        data-testid="group-photo-input"
        onChange={handleFileChange}
        ref={inputRef}
        type="file"
      />
    </div>
  );
}
