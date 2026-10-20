import * as React from "react";

import { GroupLetterAvatar } from "@/features/home/ui/ConversationLetterAvatar";
import { useAvatarUpload } from "@/features/profile/useAvatarUpload";
import { Button } from "@/shared/ui/button";
import { Spinner } from "@/shared/ui/spinner";

/**
 * Group photo on top of the group edit dialog: the group's avatar (photo, or
 * the letter on color) with Change photo and Remove photo. Uploads go
 * through the same path and image types as the profile photo; the parent
 * sends the resulting URL with Save changes.
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
  const { errorMessage, handleFileChange, inputRef, isUploading, openPicker } =
    useAvatarUpload({
      fallbackErrorMessage: "Could not upload that photo.",
      onUploadSuccess: onPictureChange,
    });

  React.useEffect(() => {
    onUploadingChange?.(isUploading);
  }, [isUploading, onUploadingChange]);

  const busy = disabled || isUploading;

  return (
    <div className="flex flex-col items-center gap-3" data-testid="group-photo">
      <div className="relative">
        <GroupLetterAvatar
          channelId={channelId}
          isPrivate={isPrivate}
          label={label}
          picture={picture || null}
          size="xl"
          testId="group-photo-avatar"
        />
        {isUploading ? (
          <span
            aria-label="Uploading photo"
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
          Change photo
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
            Remove photo
          </Button>
        ) : null}
      </div>
      {errorMessage ? (
        <p className="text-sm text-destructive" data-testid="group-photo-error">
          {errorMessage}
        </p>
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
