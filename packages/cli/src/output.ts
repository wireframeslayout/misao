import type { CliIo, CliOutput } from './cli-io.js';
import type { CliError } from './errors.js';

export function writeLine(stream: CliOutput, text: string): void {
  stream.write(`${text}\n`);
}

/** 1 つの JSON 文書として stdout に出す。 */
export function writeJson(io: CliIo, value: unknown): void {
  writeLine(io.stdout, JSON.stringify(value, null, 2));
}

/** NDJSON の 1 行として stdout に出す。 */
export function writeJsonLine(io: CliIo, value: unknown): void {
  writeLine(io.stdout, JSON.stringify(value));
}

/** エラーを stderr に出す。--json のときは {"error":{code,message,candidates?}}。 */
export function renderError(io: CliIo, error: CliError, isJson: boolean): void {
  if (isJson) {
    const body = { code: error.kind, message: error.message, ...(error.candidates ? { candidates: error.candidates } : {}) };
    writeLine(io.stderr, JSON.stringify({ error: body }));
    return;
  }
  writeLine(io.stderr, `[misao] ${error.message}`);
  for (const c of error.candidates ?? []) writeLine(io.stderr, `  ${c.paneId}  ${c.name}`);
}
