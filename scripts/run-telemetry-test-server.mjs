/**
 * Optional: run Aelio on port 3002 (when 3001 is busy).
 * Prefer: AELIO_CONFIG=./config.sunjet-test.yaml pnpm --filter @aelio/server dev
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { spawn } from 'node:child_process';

const root = resolve(import.meta.dirname, '..');
const src = resolve(root, 'config.sunjet-test.yaml');
const tmp = resolve(root, 'config.telemetry-test.yaml');
const yaml = readFileSync(src, 'utf8').replace(/^(\s*port:\s*)\d+\s*$/m, '$13002');
writeFileSync(tmp, yaml);

const child = spawn('npx', ['tsx', 'src/main.ts'], {
  cwd: resolve(root, 'apps/server'),
  env: {
    ...process.env,
    AELIO_CONFIG: tmp,
    AELIO_SDK_SECRET: 'test-secret',
  },
  stdio: 'inherit',
});

child.on('exit', (code) => process.exit(code ?? 0));