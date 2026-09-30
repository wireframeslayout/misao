import { chmodSync } from 'node:fs';

chmodSync(new URL('../packages/cli/dist/main.js', import.meta.url), 0o755);
