#!/usr/bin/env node
// Sign the registry document into the envelope Orivo fetches.
//
//   pnpm registry:sign                     sign index.document.json -> index.v1.json
//   pnpm registry:sign --key <pem>         sign with another PKCS#8 PEM key
//
// The document is carried as a *string* inside the envelope: the signature
// covers the bytes the publisher actually wrote, so there is no JSON
// canonicalisation rule that both sides would have to implement identically.
//
// What is signed (mirrors `index_digest` in Orivo's `plugin_index.rs`):
//   sha256("orivo-plugin-registry-index-v1\0" || document-bytes)
// Ed25519 takes that 32-byte digest as its message. The domain tag is what
// keeps an index signature from ever being accepted as a package signature
// or the other way round - `manifest.json` is JSON and cannot start with it.

import { createHash, createPrivateKey, createPublicKey, sign, verify } from 'node:crypto';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';

import { SIGNING_CONTEXT, validateDocument } from './lib.mjs';

const HERE = import.meta.dirname;
const DEFAULT_KEY = path.resolve(HERE, '..', 'keys', 'orivo-release.key');
const DOCUMENT_FILE = path.resolve(HERE, '..', 'index.document.json');
const ENVELOPE_FILE = path.resolve(HERE, '..', 'index.v1.json');

function fail(message, hints = []) {
  console.error(`\nsign-index failed: ${message}`);
  for (const hint of hints) console.error(`  - ${hint}`);
  console.error('');
  process.exit(1);
}

/** `--key <path>` overrides the default key location. */
function resolveKeyFile(argv) {
  const flag = argv.indexOf('--key');
  return flag === -1 ? DEFAULT_KEY : path.resolve(argv[flag + 1] ?? '');
}

const keyFile = resolveKeyFile(process.argv.slice(2));
if (!existsSync(DOCUMENT_FILE)) fail(`missing ${DOCUMENT_FILE}`);
if (!existsSync(keyFile)) {
  fail(`no signing key at ${keyFile}`, ["copy Orivo's release key there (see docs/plugin-registry.md)"]);
}

const privateKey = createPrivateKey(readFileSync(keyFile));
if (privateKey.asymmetricKeyType !== 'ed25519') {
  fail(`signing key is ${privateKey.asymmetricKeyType}, not ed25519`);
}
const publicKey = createPublicKey(privateKey);

const document = readFileSync(DOCUMENT_FILE, 'utf8');
let parsed;
try {
  parsed = JSON.parse(document);
} catch (error) {
  fail(`${path.basename(DOCUMENT_FILE)} is not valid JSON: ${error.message}`);
}

const problems = validateDocument(parsed, Date.now());
if (problems.length > 0) fail('the document would be refused by Orivo', problems);

const digest = createHash('sha256').update(SIGNING_CONTEXT).update(document, 'utf8').digest();
const signature = sign(null, digest, privateKey);
if (!verify(null, digest, publicKey, signature)) {
  fail('the freshly made signature did not verify against its own public key');
}

writeFileSync(
  ENVELOPE_FILE,
  `${JSON.stringify({ formatVersion: 1, document, signature: signature.toString('base64') }, null, 2)}\n`,
);

const { x } = publicKey.export({ format: 'jwk' });
console.log(`\nsigned ${path.relative(process.cwd(), DOCUMENT_FILE)} -> ${path.relative(process.cwd(), ENVELOPE_FILE)}`);
console.log(`  key:              ${path.relative(process.cwd(), keyFile)}`);
console.log(`  public key (b64): ${Buffer.from(x, 'base64url').toString('base64')}`);
console.log(`  document sha256:  ${digest.toString('hex')}`);
console.log(`  sequence:         ${parsed.sequence}`);
console.log(`  expires:          ${new Date(parsed.expiresAtEpochMs).toISOString()}`);
console.log(`  plugins:          ${parsed.plugins.length}`);
console.log('');
