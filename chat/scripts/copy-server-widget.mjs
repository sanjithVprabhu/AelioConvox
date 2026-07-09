import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Build-time coupling: the @aelio/chat IIFE build is copied into the server's
// static dir so the server can serve it at /widget.js. `chat/` and `server/` are
// root siblings, so this relative hop stays valid. If either dir moves, update this.
const root = dirname(fileURLToPath(import.meta.url));
const packageRoot = resolve(root, '..');
const source = resolve(packageRoot, 'dist/widget.js');
const target = resolve(packageRoot, '../server/public/widget.js');

mkdirSync(dirname(target), { recursive: true });
copyFileSync(source, target);
