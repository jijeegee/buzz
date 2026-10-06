import type { Page } from "@playwright/test";
import { installMockBridge } from "./bridge";

/** Supply the existing non-token AUTH path at the mock native boundary. */
export async function installChatListBridge(page: Page) {
  await page.addInitScript(() => {
    const w = window as unknown as {
      __TAURI_INTERNALS__: {
        invoke: (command: string, args?: unknown) => Promise<unknown>;
      };
    };
    let invoke: typeof w.__TAURI_INTERNALS__.invoke;
    w.__TAURI_INTERNALS__ = {} as typeof w.__TAURI_INTERNALS__;
    Object.defineProperty(w.__TAURI_INTERNALS__, "invoke", {
      configurable: true,
      set: (value) => {
        invoke = value;
      },
      get: () => async (command: string, args?: unknown) => {
        if (command === "get_ws_auth_frame") return null;
        if (command === "is_shared_identity") return false;
        if (command === "get_token_auth_status")
          return {
            supported: false,
            origin: "http://localhost:3000",
            state: "signed_out",
            providers: [],
            principal: null,
            deviceId: null,
            reason: null,
            keyBackupSupported: false,
            keyBackup: false,
          };
        return invoke(command, args);
      },
    });
  });
  await installMockBridge(page);
}
