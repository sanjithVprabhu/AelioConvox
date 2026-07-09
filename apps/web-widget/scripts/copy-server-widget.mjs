import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(fileURLToPath(import.meta.url));
const packageRoot = resolve(root, '..');
const source = resolve(packageRoot, 'dist/widget.js');
const target = resolve(packageRoot, '../server/public/widget.js');

mkdirSync(dirname(target), { recursive: true });
copyFileSync(source, target);
