import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { after, before, test } from 'node:test';
import { ConfigError, loadConfig } from '../../src/config/load.js';

let root: string;
let homeDir: string;
let seq = 0;

before(() => {
  root = mkdtempSync(path.join(tmpdir(), 'misao-config-'));
  homeDir = path.join(root, 'home');
  mkdirSync(path.join(homeDir, '.misao'), { recursive: true });
});

after(() => {
  rmSync(root, { recursive: true, force: true });
});

function writeConfig(content: unknown, dir = root): string {
  const file = path.join(dir, `c${seq++}.json`);
  writeFileSync(file, typeof content === 'string' ? content : JSON.stringify(content));
  return file;
}

function emptyHome(): string {
  return path.join(root, `empty-home-${seq++}`);
}

test('ファイル無しは既定値', () => {
  const home = emptyHome();
  const loaded = loadConfig({ env: {}, homeDir: home });
  assert.equal(loaded.source, null);
  assert.deepEqual(loaded.warnings, []);
  assert.deepEqual(loaded.config, {
    keys: { prefix: 0x1e, detach: 0x64, next: 0x6e, prev: 0x70, list: 0x6c },
    scrollback: 5000,
    rings: { rawBytes: 1048576, linesBytes: 65536, events: 1000 },
    logLevel: 'info',
    socket: path.join(home, '.misao', 'misao.sock'),
  });
});

test('探索順: flag > $MISAO_CONFIG > $MISAO_DIR > ~/.misao', () => {
  const flag = writeConfig({ scrollback: 1 });
  const envFile = writeConfig({ scrollback: 2 });
  const dir = path.join(root, 'mdir');
  mkdirSync(dir);
  writeFileSync(path.join(dir, 'misao.json'), JSON.stringify({ scrollback: 3 }));
  const home = path.join(root, 'home');
  writeFileSync(path.join(home, '.misao', 'misao.json'), JSON.stringify({ scrollback: 4 }));

  const pick = (input: Parameters<typeof loadConfig>[0]): number =>
    loadConfig(input).config.scrollback;
  assert.equal(pick({ flagPath: flag, env: { MISAO_CONFIG: envFile, MISAO_DIR: dir }, homeDir: home }), 1);
  assert.equal(pick({ env: { MISAO_CONFIG: envFile, MISAO_DIR: dir }, homeDir: home }), 2);
  assert.equal(pick({ env: { MISAO_DIR: dir }, homeDir: home }), 3);
  assert.equal(pick({ env: {}, homeDir: home }), 4);
  rmSync(path.join(home, '.misao', 'misao.json'));
});

test('$MISAO_DIR に無ければ次の候補へ進む', () => {
  const loaded = loadConfig({ env: { MISAO_DIR: path.join(root, 'nonexistent') }, homeDir: emptyHome() });
  assert.equal(loaded.source, null);
});

test('明示指定のファイルが無ければエラー', () => {
  const missing = path.join(root, 'missing.json');
  assert.throws(() => loadConfig({ flagPath: missing, env: {}, homeDir: emptyHome() }), ConfigError);
  assert.throws(
    () => loadConfig({ env: { MISAO_CONFIG: missing }, homeDir: emptyHome() }),
    /missing\.json/,
  );
});

test('ENOENT 以外の読み込みエラーは失敗（ディレクトリを指定）', () => {
  assert.throws(() => loadConfig({ flagPath: root, env: {}, homeDir: emptyHome() }), ConfigError);
});

test('JSON 構文エラー', () => {
  const file = writeConfig('{ nope');
  assert.throws(() => loadConfig({ flagPath: file, env: {}, homeDir: emptyHome() }), /invalid JSON/);
});

test('型違い・値域外はエラー', () => {
  for (const bad of [{ scrollback: 'many' }, { scrollback: 0 }, { logLevel: 'trace' }, { keys: { detach: 'xx' } }, []]) {
    const file = writeConfig(bad);
    assert.throws(
      () => loadConfig({ flagPath: file, env: {}, homeDir: emptyHome() }),
      ConfigError,
      JSON.stringify(bad),
    );
  }
});

test('ネストした未知キーは警告して無視', () => {
  const file = writeConfig({ scrollback: 10, extra: 1, rings: { events: 5, bogus: true }, keys: { zzz: 'q' } });
  const loaded = loadConfig({ flagPath: file, env: {}, homeDir: emptyHome() });
  assert.equal(loaded.config.scrollback, 10);
  assert.equal(loaded.config.rings.events, 5);
  assert.equal(loaded.warnings.length, 3);
  for (const key of ['extra', 'rings.bogus', 'keys.zzz']) {
    assert.ok(loaded.warnings.some((w) => w.includes(`"${key}"`)), key);
  }
  assert.ok(!('extra' in loaded.config));
});

test('未知キーと型違いが併存したらエラー', () => {
  const file = writeConfig({ extra: 1, scrollback: 'x' });
  assert.throws(() => loadConfig({ flagPath: file, env: {}, homeDir: emptyHome() }), ConfigError);
});

test('キー重複・prefix 衝突はエラー', () => {
  for (const keys of [{ next: 'd' }, { detach: 'C-^' }]) {
    const file = writeConfig({ keys });
    assert.throws(() => loadConfig({ flagPath: file, env: {}, homeDir: emptyHome() }), ConfigError);
  }
});

test('detach キーを変更できる', () => {
  const file = writeConfig({ keys: { detach: 'x', prefix: 'C-a' } });
  const { config } = loadConfig({ flagPath: file, env: {}, homeDir: emptyHome() });
  assert.equal(config.keys.detach, 0x78);
  assert.equal(config.keys.prefix, 0x01);
  assert.equal(config.keys.next, 0x6e);
});

test('socket は explicitPath として扱われ、$MISAO_SOCKET に負ける', () => {
  const file = writeConfig({ socket: '/tmp/from-config.sock' });
  const home = emptyHome();
  assert.equal(
    loadConfig({ flagPath: file, env: {}, homeDir: home }).config.socket,
    '/tmp/from-config.sock',
  );
  assert.equal(
    loadConfig({ flagPath: file, env: { MISAO_SOCKET: '/tmp/from-env.sock' }, homeDir: home }).config.socket,
    '/tmp/from-env.sock',
  );
});

test('$MISAO_DIR の ~/ は設定ファイル探索でもソケットと同じく展開される', () => {
  const home = emptyHome();
  mkdirSync(path.join(home, 'd'), { recursive: true });
  writeFileSync(path.join(home, 'd', 'misao.json'), JSON.stringify({ scrollback: 7 }));
  const { config, source } = loadConfig({ env: { MISAO_DIR: '~/d' }, homeDir: home });
  assert.equal(source, path.join(home, 'd', 'misao.json'));
  assert.equal(config.scrollback, 7);
  assert.equal(config.socket, path.join(home, 'd', 'misao.sock'));
});

test('相対パスの $MISAO_DIR は ConfigError', () => {
  assert.throws(
    () => loadConfig({ env: { MISAO_DIR: 'rel' }, homeDir: emptyHome() }),
    (error: unknown) => error instanceof ConfigError && error.message.includes('MISAO_DIR'),
  );
});

test('$MISAO_SOCKET が不正なときは原因の env 名を示す', () => {
  const file = writeConfig({});
  assert.throws(
    () => loadConfig({ flagPath: file, env: { MISAO_SOCKET: 'rel.sock' }, homeDir: emptyHome() }),
    (error: unknown) => error instanceof ConfigError && error.message.includes('MISAO_SOCKET'),
  );
});

test('長すぎる socket は ConfigError（設定ファイルのパスを含む）', () => {
  const file = writeConfig({ socket: `/${'a'.repeat(120)}.sock` });
  assert.throws(
    () => loadConfig({ flagPath: file, env: {}, homeDir: emptyHome() }),
    (error: unknown) => error instanceof ConfigError && error.message.includes(file) && error.cause !== undefined,
  );
});
