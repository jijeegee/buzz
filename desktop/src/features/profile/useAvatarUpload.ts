import * as React from "react";
import { flushSync } from "react-dom";

import { resizeAvatarImage } from "@/features/profile/lib/resizeAvatarImage";
import { uploadMediaBytes } from "@/shared/api/tauri";
import { useT } from "@/shared/i18n";

const AVATAR_IMAGE_TYPES = [
  "image/gif",
  "image/jpeg",
  "image/png",
  "image/webp",
];

type UseAvatarUploadOptions = {
  fallbackErrorMessage?: string;
  onUploadStart?: (file: File) => void;
  onUploadSettled?: () => void;
  onUploadSuccess: (url: string) => void;
  processImage?: (file: File) => Promise<string>;
};

type UseAvatarUploadReturn = {
  inputRef: React.RefObject<HTMLInputElement | null>;
  isUploading: boolean;
  errorMessage: string | null;
  clearError: () => void;
  openPicker: () => void;
  uploadFile: (file: File) => Promise<void>;
  handleFileChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
};

export function useAvatarUpload({
  fallbackErrorMessage,
  onUploadStart,
  onUploadSettled,
  onUploadSuccess,
  processImage,
}: UseAvatarUploadOptions): UseAvatarUploadReturn {
  const t = useT();
  const inputRef = React.useRef<HTMLInputElement | null>(null);
  const [isUploading, setIsUploading] = React.useState(false);
  const [errorMessage, setErrorMessage] = React.useState<string | null>(null);

  const clearError = React.useCallback(() => {
    setErrorMessage(null);
  }, []);

  const openPicker = React.useCallback(() => {
    inputRef.current?.click();
  }, []);

  const uploadFile = React.useCallback(
    async (file: File) => {
      if (!AVATAR_IMAGE_TYPES.includes(file.type)) {
        setErrorMessage(t("profile.avatarUpload.wrongType"));
        return;
      }

      flushSync(() => {
        setIsUploading(true);
        setErrorMessage(null);
        onUploadStart?.(file);
      });

      try {
        if (processImage) {
          onUploadSuccess(await processImage(file));
        } else {
          // Avatars never need more than 640px; shrink before uploading.
          const upload = await resizeAvatarImage(file);
          const buffer = await upload.arrayBuffer();
          const uploaded = await uploadMediaBytes([...new Uint8Array(buffer)]);
          // The shared upload path is now generic (accepts any non-denied file),
          // so the browser-provided `file.type` check above is no longer a
          // backstop. Verify the server-detected MIME is actually an image before
          // accepting it as an avatar — defends against spoofed/blank picker MIME.
          if (!uploaded.type.startsWith("image/")) {
            setErrorMessage(t("profile.avatarUpload.wrongType"));
            return;
          }
          onUploadSuccess(uploaded.url);
        }
      } catch (error) {
        setErrorMessage(
          error instanceof Error
            ? error.message
            : (fallbackErrorMessage ?? t("profile.avatarUpload.failed")),
        );
      } finally {
        setIsUploading(false);
        onUploadSettled?.();
      }
    },
    [
      fallbackErrorMessage,
      onUploadSettled,
      onUploadStart,
      onUploadSuccess,
      processImage,
      t,
    ],
  );

  const handleFileChange = React.useCallback(
    (event: React.ChangeEvent<HTMLInputElement>) => {
      const file = event.target.files?.[0];
      event.target.value = "";

      if (!file) {
        return;
      }

      void uploadFile(file);
    },
    [uploadFile],
  );

  return {
    inputRef,
    isUploading,
    errorMessage,
    clearError,
    openPicker,
    uploadFile,
    handleFileChange,
  };
}
