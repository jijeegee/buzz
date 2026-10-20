/**
 * Shrink a picked profile or group photo before upload.
 *
 * Avatars are shown at most a few hundred pixels wide, so the original camera
 * or export resolution only costs bandwidth for every viewer. Images larger
 * than {@link AVATAR_MAX_DIMENSION} on either side (640px, Telegram's large
 * profile photo size) are scaled down (never up) and re-encoded as JPEG;
 * images with transparency stay PNG so their see-through areas survive. GIFs
 * are uploaded unchanged so animation is kept. When decoding or encoding fails
 * the original file is uploaded instead.
 */

export const AVATAR_MAX_DIMENSION = 640;
export const AVATAR_JPEG_QUALITY = 0.85;

type DecodedImage = {
  width: number;
  height: number;
  source: CanvasImageSource;
  close?: () => void;
};

type AvatarCanvas = {
  width: number;
  height: number;
  getContext: (type: "2d") => {
    drawImage: (
      image: CanvasImageSource,
      dx: number,
      dy: number,
      dw: number,
      dh: number,
    ) => void;
    getImageData: (
      sx: number,
      sy: number,
      sw: number,
      sh: number,
    ) => { data: ArrayLike<number> };
  } | null;
  toBlob: (
    callback: (blob: Blob | null) => void,
    type: string,
    quality?: number,
  ) => void;
};

export type ResizeAvatarImageDependencies = {
  decode?: (file: Blob) => Promise<DecodedImage>;
  createCanvas?: () => AvatarCanvas;
};

async function decodeWithImageBitmap(file: Blob): Promise<DecodedImage> {
  const bitmap = await createImageBitmap(file);
  return {
    width: bitmap.width,
    height: bitmap.height,
    source: bitmap,
    close: () => bitmap.close(),
  };
}

function createDomCanvas(): AvatarCanvas {
  return document.createElement("canvas") as unknown as AvatarCanvas;
}

/** Target size that fits within the max box, never larger than the source. */
export function avatarTargetSize(
  width: number,
  height: number,
  max: number = AVATAR_MAX_DIMENSION,
): { width: number; height: number } {
  const scale = Math.min(1, max / Math.max(width, height));
  return {
    width: Math.max(1, Math.round(width * scale)),
    height: Math.max(1, Math.round(height * scale)),
  };
}

function hasTransparency(data: ArrayLike<number>): boolean {
  for (let index = 3; index < data.length; index += 4) {
    if (data[index] < 255) return true;
  }
  return false;
}

function canvasToBlob(
  canvas: AvatarCanvas,
  type: string,
  quality?: number,
): Promise<Blob | null> {
  return new Promise((resolve) => canvas.toBlob(resolve, type, quality));
}

function renamed(name: string, extension: string): string {
  const base = name.replace(/\.[^./\\]*$/, "") || "avatar";
  return `${base}.${extension}`;
}

export async function resizeAvatarImage(
  file: File,
  dependencies: ResizeAvatarImageDependencies = {},
): Promise<File> {
  if (file.type === "image/gif") return file;

  let decoded: DecodedImage;
  try {
    decoded = await (dependencies.decode ?? decodeWithImageBitmap)(file);
  } catch {
    return file;
  }

  try {
    const target = avatarTargetSize(decoded.width, decoded.height);
    const needsResize =
      target.width !== decoded.width || target.height !== decoded.height;
    // A JPEG already within the box has nothing to gain from re-encoding.
    if (!needsResize && file.type === "image/jpeg") return file;

    const canvas = (dependencies.createCanvas ?? createDomCanvas)();
    canvas.width = target.width;
    canvas.height = target.height;
    const context = canvas.getContext("2d");
    if (!context) return file;
    context.drawImage(decoded.source, 0, 0, target.width, target.height);

    const transparent =
      file.type !== "image/jpeg" &&
      hasTransparency(
        context.getImageData(0, 0, target.width, target.height).data,
      );
    // A small image with transparency is already as small as we would make it.
    if (transparent && !needsResize) return file;

    const outputType = transparent ? "image/png" : "image/jpeg";
    const blob = await canvasToBlob(
      canvas,
      outputType,
      transparent ? undefined : AVATAR_JPEG_QUALITY,
    );
    if (!blob) return file;
    // Re-encoding a small opaque PNG/WebP is only worth it when it shrinks.
    if (!needsResize && blob.size >= file.size) return file;

    return new File([blob], renamed(file.name, transparent ? "png" : "jpg"), {
      type: outputType,
    });
  } catch {
    return file;
  } finally {
    decoded.close?.();
  }
}
