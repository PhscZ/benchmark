#!/usr/bin/env node
/**
 * Minimal static file server.
 *
 * The editor is plain ES modules with no build step, so a static server is all
 * that is needed. Used by `npm start` and by the end-to-end runner.
 *
 * Usage: node tools/serve.mjs [root] [port]
 */
import { createServer } from 'node:http';
import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';
import { extname, join, normalize, resolve, sep } from 'node:path';
import { pathToFileURL } from 'node:url';

const CONTENT_TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
  '.bmp': 'image/bmp',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
  '.txt': 'text/plain; charset=utf-8',
  '.md': 'text/markdown; charset=utf-8',
};

/**
 * @param {string} rootDir directory to serve
 * @returns {import('node:http').Server}
 */
export function createStaticServer(rootDir) {
  const root = resolve(rootDir);
  return createServer(async (request, response) => {
    try {
      const url = new URL(request.url, 'http://localhost');
      const relative = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, '');
      let target = join(root, relative);
      if (!target.startsWith(root + sep) && target !== root) {
        response.writeHead(403).end('Forbidden');
        return;
      }
      let info = await stat(target).catch(() => null);
      if (info?.isDirectory()) {
        target = join(target, 'index.html');
        info = await stat(target).catch(() => null);
      }
      if (!info?.isFile()) {
        response.writeHead(404, { 'content-type': 'text/plain' }).end('Not found');
        return;
      }
      response.writeHead(200, {
        'content-type': CONTENT_TYPES[extname(target).toLowerCase()] ?? 'application/octet-stream',
        'content-length': info.size,
        'cache-control': 'no-store',
      });
      createReadStream(target).pipe(response);
    } catch (error) {
      response.writeHead(500, { 'content-type': 'text/plain' }).end(`Server error: ${error.message}`);
    }
  });
}

/** @param {{rootDir?:string, port?:number}} [options] */
export function listen({ rootDir = '.', port = 8080 } = {}) {
  const server = createStaticServer(rootDir);
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, '127.0.0.1', () => resolve(server));
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const rootDir = process.argv[2] ?? '.';
  const port = Number(process.argv[3] ?? process.env.PORT ?? 8080);
  const server = await listen({ rootDir, port });
  const address = server.address();
  process.stdout.write(`Raster Editor: http://127.0.0.1:${address.port}/ (serving ${resolve(rootDir)})\n`);
}
