import { defineConfig, loadEnv } from "vite";

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
        ignored: ["**/src-tauri/**"],
      },
    },
    envPrefix: ["VITE_", "TAURI_ENV_"],
    build: {
      target: isWindows ? "chrome105" : "safari13",
      minify: env.TAURI_ENV_DEBUG ? false : "esbuild",
      sourcemap: Boolean(env.TAURI_ENV_DEBUG),
      cssMinify: "esbuild",
      assetsInlineLimit: 0,
    },
  };
});
