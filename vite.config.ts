import { readFileSync } from "node:fs";
import { defineConfig, loadEnv } from "vite";

const { version } = JSON.parse(readFileSync("./package.json", "utf8")) as { version: string };

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, ".", "");
  const host = env.TAURI_DEV_HOST;
  const isWindows = env.TAURI_ENV_PLATFORM === "windows";

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
    build: {
      target: isWindows ? "chrome105" : "safari13",
      minify: env.TAURI_ENV_DEBUG ? false : "esbuild",
      sourcemap: Boolean(env.TAURI_ENV_DEBUG),
      cssMinify: "esbuild",
      assetsInlineLimit: 0,
    },
  };
});
