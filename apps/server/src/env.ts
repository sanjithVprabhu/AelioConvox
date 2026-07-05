import { existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const serverRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');

export function loadDotEnv(): void {
  const candidates = [
    resolve(process.cwd(), '.env'),
    resolve(serverRoot, '../../.env'),
    resolve(serverRoot, '../.env'),
  ];

  for (const path of candidates) {
    if (!existsSync(path)) {
      continue;
    }

    try {
      process.loadEnvFile(path);
    } catch {
      // Ignore missing or invalid .env files.
    }
    return;
  }
}