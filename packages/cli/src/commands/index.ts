import type { Command } from './command.js';
import { killCommand } from './kill.js';
import { labelCommand } from './label.js';
import { lsCommand } from './ls.js';
import { newCommand } from './new.js';
import { schemaCommand } from './schema.js';
import { screenCommand } from './screen.js';
import { sendCommand } from './send.js';
import { statusCommand } from './status.js';

export const COMMANDS: readonly Command[] = [
  lsCommand,
  newCommand,
  killCommand,
  statusCommand,
  sendCommand,
  screenCommand,
  labelCommand,
  schemaCommand,
];
