import * as readline from 'node:readline';
import type { CliIo } from './cli-io.js';

/** stderr に質問を出し、stdin から 1 行読んで y / yes なら true。入力が閉じたら false。 */
export async function confirm(io: CliIo, question: string): Promise<boolean> {
  const rl = readline.createInterface({ input: io.stdin, terminal: false });
  try {
    io.stderr.write(question);
    const answer = await new Promise<string>((resolve) => {
      rl.once('line', resolve);
      rl.once('close', () => resolve(''));
    });
    return /^y(es)?$/i.test(answer.trim());
  } finally {
    rl.close();
  }
}
