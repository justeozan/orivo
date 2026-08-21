import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
    restoreMocks: true,
    // Vitest loads `.env` the same way a build does, so a developer with a real
    // DSN ran the whole suite against the live Sentry SDK — initialising it,
    // posting envelopes, and flipping the feedback button visible locally while
    // it stayed hidden in CI. The tests describe the no-DSN state; pin it.
    env: {
      VITE_SENTRY_DSN: "",
    },
  },
});
