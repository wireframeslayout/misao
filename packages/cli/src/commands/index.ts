import type { Command } from './command.js';
import { labelCommand } from './label.js';
import { lsCommand } from './ls.js';
import { schemaCommand } from './schema.js';
import { screenCommand } from './screen.js';
import { sendCommand } from './send.js';
import { statusCommand } from './status.js';

export const COMMANDS: readonly Command[] = [
  lsCommand,
  statusCommand,
  sendCommand,
  screenCommand,
  labelCommand,
  schemaCommand,
];
