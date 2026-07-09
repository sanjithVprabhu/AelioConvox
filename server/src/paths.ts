import { existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const serverRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');

export function resolveMigrationsFolder(): string {
  const candidates = [
    process.env.AELIO_MIGRATIONS_PATH,
    resolve(serverRoot, '../packages/db/drizzle'),
    resolve(serverRoot, '../drizzle'),
    resolve(process.cwd(), 'packages/db/drizzle'),
    resolve(process.cwd(), 'drizzle'),
  ].filter((value): value is string => Boolean(value));

  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      return candidate;
    }
  }

  return resolve(serverRoot, '../packages/db/drizzle');
}

export function resolvePublicDir(): string {
  const candidates = [
    process.env.AELIO_PUBLIC_PATH,
    resolve(serverRoot, 'public'),
    resolve(process.cwd(), 'server/public'),
    resolve(process.cwd(), 'public'),
  ].filter((value): value is string => Boolean(value));

  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      return candidate;
    }
  }

  return resolve(serverRoot, 'public');
}