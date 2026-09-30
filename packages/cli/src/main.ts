#!/usr/bin/env node
import { PACKAGE } from './index.js';

if (process.argv.includes('--version')) {
  console.log(`${PACKAGE} 0.0.0`);
}
