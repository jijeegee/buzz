/// <reference types="vite/client" />

/** Crash reporting settings baked in by `vite.config.ts`; empty dsn = off. */
declare const __KUZZ_ERROR_REPORTING__: {
  dsn: string;
  environment: "prod" | "dev";
  release: string;
};
