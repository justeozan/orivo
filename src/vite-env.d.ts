/// <reference types="vite/client" />

/** Injected by vite.config.ts from package.json — see `define`. */
declare const __APP_VERSION__: string;

interface ImportMetaEnv {
  /**
   * Sentry ingest URL for crash reports and the in-app feedback form.
   * Absent in a source build: Sentry then never initialises.
   */
  readonly VITE_SENTRY_DSN?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
