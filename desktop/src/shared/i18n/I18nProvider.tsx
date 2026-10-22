import * as React from "react";
import { IntlContext, RawIntlProvider } from "react-intl";

import {
  getIntl,
  type MessageKey,
  type MessageValues,
  type PlainValues,
  subscribeIntl,
} from "./intl";

/** Re-renders the app when the language changes. */
export function I18nProvider({ children }: { children: React.ReactNode }) {
  const intl = React.useSyncExternalStore(subscribeIntl, getIntl, getIntl);
  return <RawIntlProvider value={intl}>{children}</RawIntlProvider>;
}

export type Translate = {
  (key: MessageKey, values?: PlainValues): string;
  (key: MessageKey, values: MessageValues): React.ReactNode;
};

/**
 * `const t = useT(); t("nav.chats")`. Works without the provider (tests,
 * isolated renders) by falling back to the app-wide translator.
 */
export function useT(): Translate {
  const intl = React.useContext(IntlContext) ?? getIntl();
  return React.useCallback(
    ((key: MessageKey, values?: MessageValues) =>
      intl.formatMessage({ id: key }, values)) as unknown as Translate,
    [intl],
  );
}
