import { defineConfig } from 'tsup';

// Bundle the internal @aelio/protocol package into the published artifact so
// `@aelio/sdk` installs cleanly from npm with no workspace dependencies — only
// its real runtime deps (`ws`, `zod`) remain external.
export default defineConfig({
  entry: ['src/index.ts'],
  format: ['esm'],
  // resolve inlines @aelio/protocol's types into the emitted .d.ts so the
  // published package needs no workspace dependency at type-check time either.
  dts: { resolve: ['@aelio/protocol'] },
  clean: true,
  sourcemap: true,
  noExternal: ['@aelio/protocol'],
});
