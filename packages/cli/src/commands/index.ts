import type { Command } from './command.js';
import { attachCommand } from './attach.js';
import { eventsCommand } from './events.js';
import { killCommand } from './kill.js';
import { labelCommand } from './label.js';
import { lsCommand } from './ls.js';
import { newCommand } from './new.js';
import { schemaCommand } from './schema.js';
import { screenCommand } from './screen.js';
import { sendCommand } from './send.js';
import { serveCommand } from './serve.js';
import { statusCommand } from './status.js';
import { tailCommand } from './tail.js';

export const COMMANDS: readonly Command[] = [
  lsCommand,
  attachCommand,
  newCommand,
  killCommand,
  statusCommand,
  sendCommand,
  screenCommand,
  tailCommand,
  eventsCommand,
  labelCommand,
  schemaCommand,
  serveCommand,
];
