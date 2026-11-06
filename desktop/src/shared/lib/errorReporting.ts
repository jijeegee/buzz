import {
  captureException,
  dedupeIntegration,
  eventFiltersIntegration,
  globalHandlersIntegration,
  init,
  linkedErrorsIntegration,
} from "@sentry/browser";

import { createReportGate, scrubEvent } from "@/shared/lib/errorReportScrub";

/**
 * Crash and error reporting for the webview, off unless the build sets
 * `KUZZ_SENTRY_DSN_WEB` (baked in by `vite.config.ts`). Only uncaught errors and the root error
 * boundary report: no breadcrumbs (console output can hold message text),
 * no sessions, no request data, and every event goes through `scrubEvent`.
 */
let enabled = false;

export function initErrorReporting(): void {
  const { dsn, environment, release } = __KUZZ_ERROR_REPORTING__;
  if (!dsn || enabled) return;
  const admit = createReportGate();
  init({
    dsn,
    release,
    environment,
    dataCollection: {
      userInfo: false,
      cookies: false,
      httpHeaders: false,
      httpBodies: [],
      urlQueryParams: false,
    },
    maxBreadcrumbs: 0,
    defaultIntegrations: false,
    integrations: [
      globalHandlersIntegration(),
      linkedErrorsIntegration(),
      dedupeIntegration(),
      eventFiltersIntegration(),
    ],
    beforeSend(event) {
      const scrubbed = scrubEvent(event);
      return admit(scrubbed) ? scrubbed : null;
    },
  });
  enabled = true;
}

/** Reports an error caught by the app, such as the root error boundary. */
export function reportError(error: unknown): void {
  if (enabled) captureException(error);
}
