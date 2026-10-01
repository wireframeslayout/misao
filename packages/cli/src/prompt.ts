import * as readline from 'node:readline';
import type { CliIo } from './cli-io.js';

/** stderr に質問を出し、stdin から 1 行読む。入力が閉じたら null。 */
export async function askLine(io: CliIo, question: string): Promise<string | null> {
  const rl = readline.createInterface({ input: io.stdin, terminal: false });
  try {
    io.stderr.write(question);
    return await new Promise<string | null>((resolve) => {
      rl.once('line', resolve);
      rl.once('close', () => resolve(null));
    });
  } finally {
    rl.close();
  }
}

/** y / yes なら true。それ以外と、入力が閉じた場合は false。 */
export async function confirm(io: CliIo, question: string): Promise<boolean> {
  const answer = await askLine(io, question);
  return answer !== null && /^y(es)?$/i.test(answer.trim());
}
