#!/usr/bin/env node

/**
 * Hand an Android App Bundle to Google Play, and bring back the APK Play signs.
 *
 *   GOOGLE_PLAY_ACCESS_TOKEN=… node scripts/google-play-release.mjs <bundle.aab> <version> <apk-out>
 *
 * The release workflow runs it with a short-lived token for Orivo's release
 * service account, obtained through Workload Identity Federation, so no key is
 * stored anywhere. Locally, the token of an account with access to the app
 * works too (`gcloud auth application-default print-access-token`), with
 * GOOGLE_PLAY_QUOTA_PROJECT naming the Cloud project its quota is billed to.
 *
 * Play signs Orivo with a key only Play holds. The APK a GitHub release offers
 * therefore has to be one Play signed: a build signed with the upload key would
 * be another app to Android, and a sideloaded install and a Play install would
 * refuse each other's updates. So:
 *
 * 1. The bundle goes to the internal testing track, as a completed release —
 *    except while the app has never been published, when Play accepts drafts
 *    only and the rollout is pressed in the Play Console.
 * 2. The universal APK Play generates from that bundle is written to <apk-out>.
 *
 * Unless Play's automatic protection is on. It rewrites the APKs Play
 * generates so that an install that did not come from the Play Store closes
 * itself and sends the user to Play (`com.pairip.licensecheck`), which is the
 * opposite of what a GitHub download is for. The script refuses such an APK
 * rather than publish one that cannot open.
 */

import { execFileSync } from "node:child_process";
import { readFileSync, rmSync, writeFileSync } from "node:fs";

const PACKAGE = process.env.GOOGLE_PLAY_PACKAGE || "run.evey.orivo";
const TRACK = process.env.GOOGLE_PLAY_TRACK || "internal";
const API = `https://androidpublisher.googleapis.com/androidpublisher/v3/applications/${PACKAGE}`;
const UPLOAD = `https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications/${PACKAGE}`;
const DRAFT_APP = /only releases with status draft may be created on draft app/i;
const ALREADY_USED = /version code .*already been used|already been used/i;
/** Play generates the APKs once it has processed the bundle: minutes, not seconds. */
const GENERATED_APK_DEADLINE_MS = 20 * 60_000;
const GENERATED_APK_POLL_MS = 20_000;

const [bundlePath, version, apkOut] = process.argv.slice(2);
if (!bundlePath || !version || !apkOut) {
  console.error("usage: node scripts/google-play-release.mjs <bundle.aab> <version> <apk-out>");
  process.exit(2);
}
const token = process.env.GOOGLE_PLAY_ACCESS_TOKEN;
if (!token) {
  console.error("GOOGLE_PLAY_ACCESS_TOKEN is not set.");
  process.exit(2);
}

function headers(extra = {}) {
  const quotaProject = process.env.GOOGLE_PLAY_QUOTA_PROJECT;
  return {
    Authorization: `Bearer ${token}`,
    ...(quotaProject ? { "x-goog-user-project": quotaProject } : {}),
    ...extra,
  };
}

async function call(method, url, { json, body, extraHeaders = {} } = {}) {
  const response = await fetch(url, {
    method,
    headers: headers(json === undefined ? extraHeaders : { "Content-Type": "application/json", ...extraHeaders }),
    body: json === undefined ? body : JSON.stringify(json),
  });
  if (!response.ok) {
    let message = await response.text();
    try {
      message = JSON.parse(message).error.message;
    } catch {
      // Not Google's JSON error shape: the text says it as well as anything.
    }
    throw new Error(`${method} ${new URL(url).pathname}: HTTP ${response.status}: ${message}`);
  }
  return response;
}

async function callJson(method, url, options) {
  const text = await (await call(method, url, options)).text();
  return text ? JSON.parse(text) : {};
}

/** Resumable, because a bundle is tens of megabytes. */
async function uploadBundle(editId, bytes) {
  const session = await call("POST", `${UPLOAD}/edits/${editId}/bundles?uploadType=resumable`, {
    extraHeaders: {
      "X-Upload-Content-Type": "application/octet-stream",
      "X-Upload-Content-Length": String(bytes.length),
    },
  });
  const location = session.headers.get("location");
  if (!location) throw new Error("Play opened no upload session for the bundle.");
  const uploaded = await call("PUT", location, {
    body: bytes,
    extraHeaders: { "Content-Type": "application/octet-stream" },
  });
  return JSON.parse(await uploaded.text());
}

/** One edit: the bundle, then a release of it on the track, then the commit. */
async function release(status) {
  const { id: editId } = await callJson("POST", `${API}/edits`, { json: {} });
  try {
    const bundle = await uploadBundle(editId, readFileSync(bundlePath));
    const versionCode = String(bundle.versionCode);
    await callJson("PUT", `${API}/edits/${editId}/tracks/${TRACK}`, {
      json: {
        track: TRACK,
        releases: [
          {
            name: version,
            versionCodes: [versionCode],
            status,
            releaseNotes: [
              {
                language: "en-US",
                text: `Orivo ${version}. What changed: https://github.com/justeozan/orivo/releases/tag/v${version}`,
              },
            ],
          },
        ],
      },
    });
    await callJson("POST", `${API}/edits/${editId}:commit`);
    return versionCode;
  } catch (error) {
    // An edit that is not committed changes nothing; deleting it frees the
    // version code for the retry.
    await call("DELETE", `${API}/edits/${editId}`).catch(() => {});
    throw error;
  }
}

async function universalApkDownloadId(versionCode) {
  const deadline = Date.now() + GENERATED_APK_DEADLINE_MS;
  for (;;) {
    try {
      const { generatedApks = [] } = await callJson("GET", `${API}/generatedApks/${versionCode}`);
      const downloadId = generatedApks.find((apk) => apk.generatedUniversalApk)?.generatedUniversalApk.downloadId;
      if (downloadId) return downloadId;
    } catch (error) {
      if (Date.now() > deadline) throw error;
    }
    if (Date.now() > deadline) {
      throw new Error(`Play generated no universal APK for version code ${versionCode} in time.`);
    }
    await new Promise((resolve) => setTimeout(resolve, GENERATED_APK_POLL_MS));
  }
}

/** Tauri's own rule: major × 1,000,000 + minor × 1,000 + patch. */
function versionCodeOf(name) {
  const [major, minor, patch] = name.split("-")[0].split(".").map(Number);
  return String(major * 1_000_000 + minor * 1_000 + patch);
}

let versionCode;
try {
  versionCode = await release("completed");
} catch (error) {
  if (ALREADY_USED.test(error.message)) {
    // A re-run of a release whose bundle Play already has: Play takes a
    // version code once, so the APK it generated then is fetched again.
    versionCode = versionCodeOf(version);
    console.log(`Google Play already has version code ${versionCode}; fetching the APK it generated from it.`);
  } else if (DRAFT_APP.test(error.message)) {
    console.log(
      "Orivo has never been published on Google Play, and Play takes only drafts until it has: the release waits in the Play Console for its rollout.",
    );
    versionCode = await release("draft");
  } else {
    throw error;
  }
}
console.log(`Orivo ${version} (version code ${versionCode}) is on the ${TRACK} track.`);

const downloadId = await universalApkDownloadId(versionCode);
const apk = await call(
  "GET",
  `${API}/generatedApks/${versionCode}/downloads/${encodeURIComponent(downloadId)}:download?alt=media`,
);
writeFileSync(apkOut, Buffer.from(await apk.arrayBuffer()));

// The binary manifest stores its strings as UTF-16.
const manifest = execFileSync("unzip", ["-p", apkOut, "AndroidManifest.xml"], { maxBuffer: 64 * 1024 * 1024 });
if (manifest.includes(Buffer.from("com.pairip", "utf16le"))) {
  rmSync(apkOut);
  console.error(
    "Google Play's automatic protection is on: the APK Play generated closes itself when it was not installed from the Play Store, so it cannot be offered for download. Turn it off in the Play Console (App integrity > Automatic protection); the next bundle Play receives is generated without it.",
  );
  process.exit(1);
}
console.log(`Wrote the APK Play signed to ${apkOut}.`);
