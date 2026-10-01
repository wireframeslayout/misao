import * as readline from 'node:readline';
import type { CliIo } from './cli-io.js';

/** stderr に質問を出し、stdin から 1 行読む。入力が閉じたら、または signal で中断したら null。 */
export async function askLine(io: CliIo, question: string, signal?: AbortSignal): Promise<string | null> {
  const rl = readline.createInterface({ input: io.stdin, terminal: false });
  const abort = (): void => rl.close();
  try {
    io.stderr.write(question);
    return await new Promise<string | null>((resolve) => {
      rl.once('line', resolve);
      rl.once('close', () => resolve(null));
      if (signal?.aborted) rl.close();
      signal?.addEventListener('abort', abort, { once: true });
    });
  } finally {
    signal?.removeEventListener('abort', abort);
    rl.close();
  }
}

/** y / yes なら true。それ以外と、入力が閉じた場合は false。 */
export async function confirm(io: CliIo, question: string): Promise<boolean> {
  const answer = await askLine(io, question);
  return answer !== null && /^y(es)?$/i.test(answer.trim());
}
