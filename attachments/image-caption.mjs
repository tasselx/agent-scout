#!/usr/bin/env node
/**
 * image-caption — Windsurf/Devin server-side image captioning CLI + library.
 *
 * Calls the `GetImageCaption` RPC on the same ApiServerService that
 * windsurf-search uses for GetWebSearchResults. Reverse-engineered from the
 * language_server_macos_arm binary's proto descriptor:
 *
 *   POST https://server.codeium.com/exa.api_server_pb.ApiServerService/GetImageCaption
 *   Connect-JSON, session-token auth via metadata.apiKey.
 *
 *   GetImageCaptionRequest:
 *     1 metadata  (Metadata: apiKey, ideName, ideVersion, ...)
 *     2 image     (ImageData: { base64Data, mimeType, caption? })
 *     3 messageText (string — your question / instruction about the image)
 *   GetImageCaptionResponse:
 *     1 caption   (string — the model's analysis)
 *
 * Zero runtime dependencies. Node >= 20 (built-in fetch + readFile).
 *
 * Usage:
 *   node image-caption.mjs <image-path> [--question "..."] [--mime image/png]
 *                              [--api-key <token>] [--host server.codeium.com]
 *                              [--json]
 *
 * Auth resolution (reuses windsurf-search.mjs):
 *   1. --api-key <key>
 *   2. WINDSURF_API_KEY / WINDSURFAPI_CODEIUM_API_KEY env
 *   3. key file: ~/.config/windsurf-search/api-key (and legacy candidates)
 */
import { readFile } from 'node:fs/promises';
import { resolveApiKey } from './windsurf-search.mjs';

const IMAGE_CAPTION_PATH = '/exa.api_server_pb.ApiServerService/GetImageCaption';
const SERVER_HOSTS = ['server.codeium.com', 'server.self-serve.windsurf.com'];
const DEFAULT_TIMEOUT_MS = 30_000;

const EXT_TO_MIME = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  ico: 'image/x-icon',
};

// Injected fetch seam for tests.
let fetchImpl = fetch;
export function __setImageCaptionFetchForTest(fn) {
  fetchImpl = fn || fetch;
}

// ─── pure helpers (unit-testable) ──────────────────────────────────────

/** Build the GetImageCaption Connect-JSON request body. */
export function buildCaptionRequestBody(apiKey, base64Data, { mimeType = 'image/png', messageText = '' } = {}) {
  const data = String(base64Data || '').replace(/^data:[^;]+;base64,/, '').trim();
  if (!data) throw new Error('image-caption: empty image data');
  const mt = String(mimeType || '').trim() || 'image/png';
  return {
    metadata: {
      apiKey,
      ideName: 'windsurf',
      ideVersion: '1.9600.41',
      extensionName: 'windsurf',
      extensionVersion: '1.9600.41',
      locale: 'en',
    },
    image: { base64Data: data, mimeType: mt },
    ...(messageText ? { messageText: String(messageText) } : {}),
  };
}

/** Guess a mime type from a filename/path extension. */
export function mimeFromPath(filePath) {
  const m = String(filePath || '').toLowerCase().match(/\.([a-z0-9]+)$/);
  return m ? EXT_TO_MIME[m[1]] || 'image/png' : 'image/png';
}

/** Read an image file from disk and return raw base64 (no data: prefix). */
export async function fileToBase64(filePath) {
  const buf = await readFile(filePath);
  return buf.toString('base64');
}

// ─── network call ──────────────────────────────────────────────────────

async function postJson(fetchFn, host, path, body, { timeoutMs = DEFAULT_TIMEOUT_MS } = {}) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetchFn(`https://${host}${path}`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Connect-Protocol-Version': '1',
        Accept: 'application/json',
        'User-Agent': 'windsurf/1.9600.41',
      },
      body: JSON.stringify(body),
      signal: controller.signal,
    });
    const raw = await response.text();
    if (response.status >= 400) {
      throw new Error(`GetImageCaption ${host} -> HTTP ${response.status}: ${raw.slice(0, 200)}`);
    }
    try {
      return JSON.parse(raw);
    } catch {
      throw new Error(`GetImageCaption ${host}: non-JSON response: ${raw.slice(0, 200)}`);
    }
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Caption an image via Windsurf's GetImageCaption endpoint.
 *
 * @param {string} apiKey  session token (devin-session-token$…)
 * @param {string} base64Data  raw base64 image bytes (data: prefix stripped if present)
 * @param {object} [options]
 * @param {string} [options.mimeType='image/png']
 * @param {string} [options.messageText]  question/instruction about the image
 * @param {string[]} [options.hosts]  override host fallback list
 * @param {number} [options.timeoutMs]
 * @param {Function} [options.fetchImpl]  test seam
 * @returns {Promise<{caption: string, raw: object}>}
 */
export async function captionImage(apiKey, base64Data, options = {}) {
  const body = buildCaptionRequestBody(apiKey, base64Data, options);
  const fetchFn = options.fetchImpl || fetchImpl;
  const timeoutMs = options.timeoutMs || DEFAULT_TIMEOUT_MS;
  const hosts = options.hosts && options.hosts.length ? options.hosts : SERVER_HOSTS;
  let lastError = null;
  for (const host of hosts) {
    try {
      const payload = await postJson(fetchFn, host, IMAGE_CAPTION_PATH, body, { timeoutMs });
      return { caption: payload?.caption ?? '', raw: payload };
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError || new Error('image-caption: all hosts failed');
}

// ─── CLI ───────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const positionals = [];
  const flags = {};
  for (let i = 0; i < argv.length; i += 1) {
    const token = argv[i];
    if (!token) continue;
    if (['--question', '--api-key', '--mime', '--host', '--timeout'].includes(token)) {
      flags[token.slice(2)] = argv[i + 1];
      i += 1;
    } else if (token === '--json') {
      flags.json = true;
    } else if (token === '--help' || token === '-h') {
      flags.help = true;
    } else if (token.startsWith('--')) {
      flags[token.slice(2)] = true;
    } else {
      positionals.push(token);
    }
  }
  return { positionals, flags };
}

function printUsage(stream) {
  stream.write(
    'image-caption: Windsurf/Devin image captioning (GetImageCaption)\n' +
      'usage:\n' +
      '  image-caption <image-path> [--question "..."] [--mime image/png]\n' +
      '                 [--api-key <token>] [--host server.codeium.com]\n' +
      '                 [--timeout 30000] [--json]\n' +
      '\n' +
      'reads an image file, base64-encodes it, sends it to the same\n' +
      'ApiServerService that powers windsurf-search, and prints the caption.\n' +
      '\n' +
      'with --json, prints { "caption": "..." } to stdout (errors go to stderr).\n' +
      'without --json, prints just the caption text.\n',
  );
}

export async function main(argv = process.argv.slice(2)) {
  const { positionals, flags } = parseArgs(argv);

  if (flags.help || positionals.length === 0) {
    printUsage(process.stderr);
    return flags.help ? 0 : 2;
  }

  const imagePath = positionals[0];
  const question = typeof flags.question === 'string' ? flags.question : '';
  const mimeType =
    typeof flags.mime === 'string' && flags.mime ? flags.mime : mimeFromPath(imagePath);
  const asJson = !!flags.json;

  let apiKey;
  try {
    apiKey = await resolveApiKey({ cliValue: flags['api-key'] });
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    return 1;
  }

  let base64Data;
  try {
    base64Data = await fileToBase64(imagePath);
  } catch (error) {
    process.stderr.write(
      `image-caption: failed to read ${imagePath}: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    return 1;
  }

  const options = { mimeType, messageText: question };
  if (flags.host && typeof flags.host === 'string') options.hosts = [flags.host];
  if (flags.timeout && Number(flags.timeout) > 0) options.timeoutMs = Number(flags.timeout);

  try {
    const { caption } = await captionImage(apiKey, base64Data, options);
    if (asJson) {
      process.stdout.write(`${JSON.stringify({ caption })}\n`);
    } else {
      process.stdout.write(`${caption}\n`);
    }
    return 0;
  } catch (error) {
    const msg = error instanceof Error ? error.message : String(error);
    if (asJson) {
      process.stderr.write(`${msg}\n`);
    } else {
      process.stderr.write(`image-caption: ${msg}\n`);
    }
    return 1;
  }
}

// Run when executed directly.
if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  process.exitCode = await main();
}
