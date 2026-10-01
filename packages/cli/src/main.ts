#!/usr/bin/env node
import { nodeIo } from './cli-io.js';
import { run } from './run.js';

process.exit(await run(process.argv.slice(2), nodeIo()));
