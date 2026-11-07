/** The kuzz.app home page, which lists every download. */
export const KUZZ_DOWNLOADS_URL = "https://kuzz.app";
export const KUZZ_WINDOWS_DOWNLOAD_URL = "https://kuzz.app/download/windows";
export const KUZZ_MAC_DOWNLOAD_URL = "https://kuzz.app/download/mac";
export const KUZZ_IOS_DOWNLOAD_URL = "https://kuzz.app/download/ios";
export const KUZZ_ANDROID_DOWNLOAD_URL = "https://kuzz.app/download/android";

export type KuzzDownloadPlatform = {
  operatingSystem:
    | "linux"
    | "macos"
    | "windows"
    | "ios"
    | "android"
    | "unknown";
  architecture: "arm64" | "x64" | "unknown";
};

type UserAgentData = {
  platform?: string;
  mobile?: boolean;
  getHighEntropyValues?: (
    hints: string[],
  ) => Promise<{ architecture?: string; bitness?: string }>;
};

function normalizeOperatingSystem(
  navigatorValue: Navigator,
  userAgentData?: UserAgentData,
): KuzzDownloadPlatform["operatingSystem"] {
  const userAgent = navigatorValue.userAgent.toLowerCase();
  const platform = (
    userAgentData?.platform ??
    navigatorValue.platform ??
    ""
  ).toLowerCase();

  // Compatibility tokens are treacherous: iPadOS can report MacIntel and a
  // Macintosh UA, while Android and ChromeOS expose Linux platform strings.
  // Classify mobile devices before admitting desktop-looking signals.
  const isIPadDesktopMode =
    platform === "macintel" && navigatorValue.maxTouchPoints > 1;
  if (isIPadDesktopMode || /iphone|ipad|ipod/.test(userAgent)) return "ios";
  // Fire OS identifies as Android but does not ship with Google Play.
  if (/kindle|silk/.test(userAgent)) return "unknown";
  if (/android/.test(userAgent) || platform === "android") return "android";

  const isUnsupportedDevice =
    userAgentData?.mobile === true ||
    /mobile|tablet|windows phone|iemobile|opera mini|opera mobi|webos|blackberry|bb10|kindle|silk|kaios|cros/.test(
      userAgent,
    );
  if (isUnsupportedDevice) return "unknown";

  if (
    platform === "macos" ||
    platform.startsWith("mac") ||
    userAgent.includes("macintosh")
  )
    return "macos";
  if (
    platform === "windows" ||
    platform.startsWith("win") ||
    userAgent.includes("windows nt")
  )
    return "windows";
  if (
    platform === "linux" ||
    platform.startsWith("linux") ||
    userAgent.includes("linux")
  )
    return "linux";
  return "unknown";
}

function normalizeArchitecture(
  value: string,
): KuzzDownloadPlatform["architecture"] {
  const normalized = value.toLowerCase();
  if (/arm|aarch64/.test(normalized)) return "arm64";
  if (/x86|x64|amd64|64/.test(normalized)) return "x64";
  return "unknown";
}

export async function detectKuzzDownloadPlatform(
  navigatorValue: Navigator,
): Promise<KuzzDownloadPlatform> {
  const userAgentData = (
    navigatorValue as Navigator & { userAgentData?: UserAgentData }
  ).userAgentData;
  const operatingSystem = normalizeOperatingSystem(
    navigatorValue,
    userAgentData,
  );
  if (operatingSystem === "ios" || operatingSystem === "android") {
    return { operatingSystem, architecture: "unknown" };
  }
  let architecture = normalizeArchitecture(navigatorValue.userAgent);

  if (userAgentData?.getHighEntropyValues) {
    try {
      const values = await userAgentData.getHighEntropyValues([
        "architecture",
        "bitness",
      ]);
      architecture = normalizeArchitecture(
        `${values.architecture ?? ""} ${values.bitness ?? ""}`,
      );
    } catch {
      // Privacy settings may reject high-entropy client hints; the architecture
      // from the user agent stays in place.
    }
  }

  return { operatingSystem, architecture };
}

export async function resolveKuzzDownloadUrlForPlatform(
  platform: KuzzDownloadPlatform,
): Promise<string> {
  switch (platform.operatingSystem) {
    case "ios":
      return KUZZ_IOS_DOWNLOAD_URL;
    case "android":
      return KUZZ_ANDROID_DOWNLOAD_URL;
    case "windows":
      return KUZZ_WINDOWS_DOWNLOAD_URL;
    case "macos":
      // Kuzz ships an Apple silicon build only.
      return platform.architecture === "arm64"
        ? KUZZ_MAC_DOWNLOAD_URL
        : KUZZ_DOWNLOADS_URL;
    default:
      return KUZZ_DOWNLOADS_URL;
  }
}
