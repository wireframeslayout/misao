import { MisaoClient, MisaoConnectionError } from '@misao/sdk';
import { CliError } from './errors.js';

/** デーモンへ接続する。つながらなければ daemon_unreachable (misao serve での起動を案内)。 */
export async function connectDaemon(socketPath: string): Promise<MisaoClient> {
  const client = new MisaoClient({ socketPath });
  try {
    await client.connect();
  } catch (error) {
    client.close();
    if (error instanceof MisaoConnectionError) {
      throw new CliError('daemon_unreachable', `デーモンに接続できません（${socketPath}）。misao serve で起動してください`);
    }
    throw error;
  }
  return client;
}

/** 接続して fn を実行し、どう終わっても接続を閉じる。 */
export async function withDaemon<T>(socketPath: string, fn: (client: MisaoClient) => Promise<T>): Promise<T> {
  const client = await connectDaemon(socketPath);
  try {
    return await fn(client);
  } finally {
    client.close();
  }
}
