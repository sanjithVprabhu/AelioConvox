import { existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const serverRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');

export function resolvePublicDir(): string {
  const candidates = [
    process.env.AELIO_PUBLIC_PATH,
    resolve(serverRoot, 'public'),
    resolve(process.cwd(), 'server/public'),
    resolve(process.cwd(), 'public'),
  ].filter((value): value is string => Boolean(value));

  for (const candidate of candidates) {
    const absolute = resolve(candidate);
    if (existsSync(absolute)) {
      return absolute;
    }
  }

  return resolve(serverRoot, 'public');
}
