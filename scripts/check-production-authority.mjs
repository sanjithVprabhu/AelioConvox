import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, extname, join, relative, resolve } from 'node:path';

const root = process.cwd();
const serverRoot = join(root, 'server/src');
const entry = join(serverRoot, 'main.ts');
const sourceExtensions = ['.ts', '.tsx', '.mts', '.js'];
const importPattern = /(?:import|export)\s+(?:type\s+)?(?:[^'";]+?\s+from\s+)?['"]([^'"]+)['"]/g;

function resolveLocal(from, specifier) {
  if (!specifier.startsWith('.')) return null;
  const base = resolve(dirname(from), specifier.replace(/\.js$/, ''));
  for (const suffix of ['', ...sourceExtensions, ...sourceExtensions.map((ext) => `/index${ext}`)]) {
    const candidate = `${base}${suffix}`;
    if (existsSync(candidate)) return candidate;
  }
  throw new Error(`unresolved production import ${specifier} from ${relative(root, from)}`);
}

const visited = new Set();
const stack = [entry];
while (stack.length > 0) {
  const file = stack.pop();
  if (visited.has(file)) continue;
  visited.add(file);
  const source = readFileSync(file, 'utf8');
  for (const match of source.matchAll(importPattern)) {
    const specifier = match[1];
    assert.notEqual(
      specifier,
      '@aelio/core',
      `${relative(root, file)} imports the legacy aggregate core authority`,
    );
    const local = resolveLocal(file, specifier);
    if (local) stack.push(local);
  }
}

const bannedAuthority = [
  /\bprocessTurn\b/,
  /\brunHarness\b/,
  /\brunPlanner\b/,
  /\bcreateSemanticPathwayEngine\b/,
  /\bevaluateGenericGate\b/,
  /\bderiveResolutionState\b/,
  /\bstoreCachedResponse\b/,
  /\/pathway\//,
  /runtime\/turn/,
  /runtime\/tool-loop/,
  /harness\/executor/,
];
for (const file of visited) {
  const source = readFileSync(file, 'utf8');
  for (const banned of bannedAuthority) {
    assert(!banned.test(source), `${relative(root, file)} contains banned TS authority ${banned}`);
  }
}

const edge = readFileSync(join(root, 'packages/core/src/edge.ts'), 'utf8');
for (const banned of bannedAuthority) {
  assert(!banned.test(edge), `@aelio/core/edge exposes banned authority ${banned}`);
}

const turn = readFileSync(join(serverRoot, 'conversation-turn.ts'), 'utf8');
assert.match(turn, /deps\.aelioRuntime\.submitAgentTurn\s*\(/);
assert.doesNotMatch(turn, /catch\s*\([^)]*\)\s*\{[^}]*process/i);
assert.doesNotMatch(turn, /processTurn|runHarness|pathwayEngine/);

const app = readFileSync(join(serverRoot, 'app.ts'), 'utf8');
assert.match(app, /AELIO_RUST_RUNTIME_URL/);
assert.match(app, /AELIO_RUNTIME_TOKEN/);
assert.match(app, /await aelioRuntime\.ready\(\)/);
assert.match(app, /TypeScript is not an execution authority/);

const client = readFileSync(join(serverRoot, 'aelio-runtime-client.ts'), 'utf8');
assert.match(client, /['"]\/agent\/v1\/turns['"]/);

const allServerFiles = [];
const walk = (directory) => {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) walk(path);
    else if (sourceExtensions.includes(extname(path))) allServerFiles.push(path);
  }
};
walk(serverRoot);
for (const file of allServerFiles) {
  assert.doesNotMatch(
    readFileSync(file, 'utf8'),
    /from\s+['"]@aelio\/core['"]/,
    `${relative(root, file)} bypasses the edge-only core surface`,
  );
}

console.log(`✓ Production authority graph verified (${visited.size} reachable TS modules): Rust is the only turn decision executor.`);
