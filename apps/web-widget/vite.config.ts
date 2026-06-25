import preact from '@preact/preset-vite';
import { resolve } from 'node:path';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [preact()],
  build: {
    lib: {
      entry: resolve(__dirname, 'src/widget.tsx'),
      name: 'AelioWidget',
      formats: ['iife'],
      fileName: () => 'widget.js',
    },
    outDir: resolve(__dirname, '../server/public'),
    emptyOutDir: false,
    rollupOptions: {
      output: {
        inlineDynamicImports: true,
      },
    },
  },
});