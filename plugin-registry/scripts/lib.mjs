// Shared rules for signing and verifying the Orivo registry index.
//
// The authority is `src-tauri/src/plugin_index.rs` in the Orivo repository
// (`parse_signed_index_with_key` / `parse_signed_index_document`). These are
// mirrors of it, so a document Orivo would refuse fails here first, at publish
// time, instead of silently dropping every client's Store.

/** What is prefixed to the document before it is hashed (`index_digest`). */
export const SIGNING_CONTEXT = Buffer.from('orivo-plugin-registry-index-v1\0', 'latin1');

/** Compiled into the host: an index cannot widen it. */
export const ALLOWED_HOSTS = [
  'raw.githubusercontent.com',
  'github.com',
  'objects.githubusercontent.com',
  'release-assets.githubusercontent.com',
];

export const MAX_INDEX_BYTES = 256 * 1024;
export const MAX_INDEX_ENTRIES = 256;
export const MAX_TEXT_LENGTH = 256;
export const MAX_URL_LENGTH = 512;

/** The host of an https URL, or null (`url_host`). */
export function urlHost(url) {
  if (!url.startsWith('https://')) return null;
  const authority = url.slice('https://'.length).split(/[/ ?#]/)[0];
  if (!authority || authority.includes('@')) return null;
  return authority.split(':')[0].toLowerCase() || null;
}

/** The grammar Orivo re-applies *after* the signature verifies. */
export function validateDocument(document, now) {
  const problems = [];
  const note = (message) => problems.push(message);

  if (document.schemaVersion !== 1) note('schemaVersion must be 1');
  if (!Number.isInteger(document.sequence) || document.sequence < 1) {
    note('sequence must be an integer >= 1');
  }
  if (!Number.isInteger(document.expiresAtEpochMs) || document.expiresAtEpochMs <= now) {
    note('expiresAtEpochMs must be an integer in the future');
  }
  if (!Array.isArray(document.plugins) || document.plugins.length > MAX_INDEX_ENTRIES) {
    note(`plugins must be an array of at most ${MAX_INDEX_ENTRIES} entries`);
    return problems;
  }

  const seen = new Set();
  for (const entry of document.plugins) {
    const label = `plugins[${entry?.id ?? '?'}]`;
    if (typeof entry.id !== 'string' || !/^[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*){2,}$/.test(entry.id)) {
      note(`${label}: id must be a lowercase reverse-DNS identifier`);
    }
    if (seen.has(entry.id)) note(`${label}: id is duplicated`);
    seen.add(entry.id);
    if (typeof entry.name !== 'string' || entry.name.trim() === '' || entry.name.length > MAX_TEXT_LENGTH) {
      note(`${label}: name must be 1..${MAX_TEXT_LENGTH} characters`);
    }
    if (typeof entry.name === 'string' && [...entry.name].some((c) => c < ' ' || c === '\u007f')) {
      note(`${label}: name must not contain control characters`);
    }
    if (typeof entry.summary !== 'string' || [...entry.summary].length > MAX_TEXT_LENGTH) {
      note(`${label}: summary must be at most ${MAX_TEXT_LENGTH} characters`);
    }
    if (typeof entry.version !== 'string' || !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(entry.version)) {
      note(`${label}: version must be a semantic version`);
    }
    if (typeof entry.sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(entry.sha256)) {
      note(`${label}: sha256 must be 64 lowercase hex characters`);
    }
    if (
      !Number.isInteger(entry.sizeBytes) ||
      entry.sizeBytes <= 0 ||
      entry.sizeBytes > 128 * 1024 * 1024
    ) {
      note(`${label}: sizeBytes must be an integer in (0, 128 MiB]`);
    }
    if (
      entry.minOrivoVersion !== undefined &&
      !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(entry.minOrivoVersion)
    ) {
      note(`${label}: minOrivoVersion must be a semantic version`);
    }
    if (typeof entry.url !== 'string' || entry.url.length > MAX_URL_LENGTH) {
      note(`${label}: url must be a string of at most ${MAX_URL_LENGTH} characters`);
      continue;
    }
    const host = urlHost(entry.url);
    if (host === null) note(`${label}: url must be https, without credentials`);
    else if (!ALLOWED_HOSTS.includes(host)) note(`${label}: url host ${host} is not on the allowlist`);
  }
  return problems;
}
