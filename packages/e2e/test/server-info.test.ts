import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { PROTOCOL_VERSION, methods } from '@misao/protocol';
import type { MisaoClient } from '@misao/sdk';
import { startDaemonProcess } from './helpers/daemon-process.js';
import type { DaemonProcess } from './helpers/daemon-process.js';

// Node 版と Rust 版 (MISAO_DAEMON_BIN) の両方で通る、デーモンの接続口と server.info の最小の契約。
let daemon: DaemonProcess;
let client: MisaoClient;

before(async () => {
  daemon = await startDaemonProcess();
  client = await daemon.connect();
});
after(() => daemon.stop());

test('server.info は protocol のスキーマどおりの項目を返す', async () => {
  const info = await client.request('server.info', {});
  assert.doesNotThrow(() => methods['server.info'].result.parse(info));
  assert.equal(info.protocolVersion, PROTOCOL_VERSION);
  assert.equal(info.pid, daemon.pid);
  assert.equal(info.paneCount, 0);
  assert.ok(info.eventHead >= 1, 'daemon.started が 1 件目');
  assert.ok(typeof info.version === 'string' && info.version.length > 0);
});

test('server.info の epoch は接続をまたいで同じで、uptimeSec は 0 以上の整数', async () => {
  const other = await daemon.connect();
  const [a, b] = [await client.request('server.info', {}), await other.request('server.info', {})];
  assert.equal(a.epoch, b.epoch);
  assert.ok(Number.isInteger(a.uptimeSec) && a.uptimeSec >= 0);
});

test('server.schema はプロトコル全体のスキーマを返す', async () => {
  const schema = await client.request('server.schema', {});
  assert.equal(schema.protocolVersion, PROTOCOL_VERSION);
});
