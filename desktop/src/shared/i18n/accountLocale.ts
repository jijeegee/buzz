import * as React from "react";
import { IntlContext } from "react-intl";

import { invokeTauri } from "@/shared/api/tauri";
import { getIntl } from "./intl";

/** The development pseudo-language; never saved on the account. */
const PSEUDO_LOCALE = "en-XA";

/**
 * Keeps the app language saved on the signed-in account, so text the server
 * writes (moderation notices) uses it. Runs when the account's shell mounts
 * and whenever the language changes. Failures are ignored: the server falls
 * back to English.
 */
export function useAccountLocaleSync(): void {
  const { locale } = React.useContext(IntlContext) ?? getIntl();
  React.useEffect(() => {
    if (locale === PSEUDO_LOCALE) return;
    invokeTauri<void>("sync_account_locale", { locale }).catch(() => {});
  }, [locale]);
}
