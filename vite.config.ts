import { readFileSync } from "node:fs";
import { sentryVitePlugin } from "@sentry/vite-plugin";
import { defineConfig, loadEnv } from "vite";

const { version } = JSON.parse(readFileSync("./package.json", "utf8")) as { version: string };

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, ".", "");
  const host = env.TAURI_DEV_HOST;
  const isWindows = env.TAURI_ENV_PLATFORM === "windows";
  // Only a release build with a token uploads maps. Without one — every build
  // from source, every test — nothing changes: no maps, no network call.
  const uploadSourcemaps = Boolean(env.SENTRY_AUTH_TOKEN) && !env.TAURI_ENV_DEBUG;

  return {
    clearScreen: false,
    server: {
      host: host || false,
      port: 5173,
      strictPort: true,
      hmr: host
        ? {
            protocol: "ws",
            host,
            port: 1421,
          }
        : undefined,
      watch: {
        // The workspace Cargo.toml puts the build dir at the repo root, not
        // under src-tauri, so cargo writes during `tauri dev` reach the
        // watcher and reload the page mid-build.
        ignored: ["**/src-tauri/**", "**/target/**"],
      },
    },
    envPrefix: ["VITE_", "TAURI_ENV_"],
    // The version a crash report is tagged with. Read from package.json so a
    // release cannot be stamped with a number nobody bumped.
    define: {
      __APP_VERSION__: JSON.stringify(version),
    },
    plugins: uploadSourcemaps
      ? [
          sentryVitePlugin({
            authToken: env.SENTRY_AUTH_TOKEN,
            org: "ozan-6n",
            project: "orivo",
            // The project lives in Sentry's German region, not sentry.io.
            url: "https://de.sentry.io",
            // The same string `src/sentry.ts` stamps on every report, so a stack
            // trace finds the maps of the build that produced it.
            release: { name: `orivo@${version}` },
            // The maps exist to be uploaded, not shipped: they would otherwise
            // sit inside every installer next to the code they explain.
            sourcemaps: { filesToDeleteAfterUpload: ["./dist/**/*.map"] },
            telemetry: false,
          }),
        ]
      : [],
    build: {
      target: isWindows ? "chrome105" : "safari13",
      minify: env.TAURI_ENV_DEBUG ? false : "esbuild",
      // "hidden" writes the maps without the `sourceMappingURL` comment, so the
      // shipped bundle does not point at files it no longer carries.
      sourcemap: env.TAURI_ENV_DEBUG ? true : uploadSourcemaps ? "hidden" : false,
      cssMinify: "esbuild",
      assetsInlineLimit: 0,
    },
  };
});
