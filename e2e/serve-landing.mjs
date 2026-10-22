// Serves landing/ under /zec-shielded-x402/ on 127.0.0.1, the path GitHub Pages uses.
import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

const port = Number(process.argv[2] || 4050);
const root = fileURLToPath(new URL('../landing', import.meta.url));
const base = '/zec-shielded-x402/';
const types = { '.html': 'text/html; charset=utf-8', '.css': 'text/css', '.js': 'text/javascript', '.svg': 'image/svg+xml', '.png': 'image/png', '.ico': 'image/x-icon', '.webp': 'image/webp' };

createServer(async (req, res) => {
  const path = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  if (path === '/zec-shielded-x402') { res.writeHead(301, { location: base }).end(); return; }
  if (!path.startsWith(base)) { res.writeHead(404).end('not found'); return; }
  let file = normalize(join(root, path.slice(base.length)));
  if (!file.startsWith(root)) { res.writeHead(403).end(); return; }
  try {
    if ((await stat(file)).isDirectory()) file = join(file, 'index.html');
    const body = await readFile(file);
    res.writeHead(200, { 'content-type': types[extname(file)] || 'application/octet-stream' }).end(body);
  } catch {
    res.writeHead(404).end('not found');
  }
}).listen(port, '127.0.0.1', () => console.log(`landing on http://127.0.0.1:${port}${base}`));
