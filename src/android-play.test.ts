import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * Android ships under the package of Orivo's Google Play record, and that
 * package is written in four places that must agree. Play takes a bundle only
 * under the package the record was created with. Tauri's Android identifier is
 * the package wry writes its Kotlin glue and JNI symbols into, and Gradle's
 * namespace is where MainActivity, which extends that glue, is looked for: if
 * either drifts from the others the build still succeeds, and the app crashes
 * as it starts.
 */

// Not `new URL(`../${path}`, import.meta.url)`: Vite rewrites that pattern into
// an asset lookup, which resolves a path it was not built with to `undefined`.
const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path: string): string => readFileSync(join(ROOT, path), "utf8");
const PACKAGE = "run.evey.orivo";
const ANDROID = "src-tauri/gen/android";

describe("Android Google Play build", () => {
  it("builds Android under the package of the Google Play record", () => {
    expect(JSON.parse(read("src-tauri/tauri.android.conf.json")).identifier).toBe(PACKAGE);
  });

  it("gives Gradle the same package, for the device and for the code", () => {
    const gradle = read(`${ANDROID}/app/build.gradle.kts`);
    expect(gradle).toContain(`namespace = "${PACKAGE}"`);
    expect(gradle).toContain(`applicationId = "${PACKAGE}"`);
  });

  it("keeps MainActivity in the package the generated glue is written to", () => {
    const activity = read(`${ANDROID}/app/src/main/java/${PACKAGE.replaceAll(".", "/")}/MainActivity.kt`);
    expect(activity.split("\n")[0]).toBe(`package ${PACKAGE}`);
  });
});
