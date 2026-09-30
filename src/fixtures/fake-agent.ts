// テスト用の偽エージェント。markers / question の 2 モード。
import * as fs from 'node:fs';
import { randomBytes } from 'node:crypto';

const argv = process.argv.slice(2);
const mode = argv[0];

function flag(name: string): string | undefined {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 ? argv[i + 1] : undefined;
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const out = (s: string) => process.stdout.write(s);

const ESC = '\x1b';

/** マーカーを複数 write に分割して出す (トークン途中で分割。前後に SGR を付けることがある)。 */
async function writeMarker(marker: string, decorate: boolean): Promise<void> {
  const cuts = [3, 8, marker.length - 4];
  const parts: string[] = [];
  let prev = 0;
  for (const c of cuts) {
    parts.push(marker.slice(prev, c));
    prev = c;
  }
  parts.push(marker.slice(prev));
  if (decorate) out(`${ESC}[1;32m`);
  for (const p of parts) {
    out(p);
    await sleep(2);
  }
  if (decorate) out(`${ESC}[0m`);
  out('\r\n');
}

async function noisyRound(round: number): Promise<void> {
  out(`${ESC}]0;fake-agent round ${round}\x07`);
  out(`${ESC}[31mred${ESC}[0m ${ESC}[38;5;208morange${ESC}[0m ${ESC}[38;2;10;200;120mtruecolor${ESC}[0m\r\n`);
  out(`日本語のワイド文字テスト：全角ＡＢＣ 漢字かな交じり 🎉 round=${round}\r\n`);
  out(`${ESC}[2A${ESC}[5C${ESC}[1B${ESC}[K(cursor moves)${ESC}[1B\r\n`);
  for (let pct = 0; pct <= 100; pct += 25) {
    out(`\rprogress [${'#'.repeat(pct / 5).padEnd(20, '.')}] ${pct}%`);
    await sleep(2);
  }
  out('\r\n');
  out(`${ESC}]2;title-st round ${round}${ESC}\\`); // ST 終端の OSC
}

async function markers(): Promise<void> {
  const count = Number(flag('count') ?? '1');
  const id = flag('id') ?? 'T';
  const outFile = flag('out');
  const burst = argv.includes('--burst');
  const nonces: string[] = [];
  for (let i = 0; i < count; i++) {
    await noisyRound(i);
    if (burst) {
      let chunk = '';
      for (let n = 0; n < 10000; n++) {
        chunk += `filler line ${n} ${ESC}[3${n % 8}mcolored${ESC}[0m xxxxxxxxxxxxxxxxxxxxxxxx\r\n`;
        if (n % 500 === 499) {
          out(chunk);
          chunk = '';
        }
      }
      out(chunk);
    }
    const nonce = randomBytes(4).toString('hex');
    nonces.push(nonce);
    await writeMarker(`AZITO_DONE_${id}_${nonce}`, i % 2 === 0);
  }
  const summary = `FAKE_SUMMARY ${JSON.stringify({ emitted: count, nonces })}`;
  if (outFile) fs.writeFileSync(outFile, summary + '\n');
  else process.stderr.write(summary + '\n');
}

async function question(): Promise<void> {
  const id = flag('id') ?? 'T';
  out('QUESTION: What is your favourite colour? > ');
  if (process.stdin.isTTY) process.stdin.setRawMode(true);
  const answer = await new Promise<string>((resolve) => {
    let buf = '';
    process.stdin.on('data', (d: Buffer) => {
      for (const ch of d.toString('utf8')) {
        if (ch === '\r' || ch === '\n') return resolve(buf);
        if (ch === '\x7f') buf = buf.slice(0, -1);
        else buf += ch;
      }
    });
  });
  out(`\r\nANSWER:${answer}\r\n`);
  await writeMarker(`AZITO_DONE_${id}_${randomBytes(4).toString('hex')}`, false);
  await new Promise(() => setInterval(() => undefined, 1 << 30)); // idle
}

if (mode === 'markers') await markers();
else if (mode === 'question') await question();
else {
  process.stderr.write('usage: fake-agent (markers --count N --id T [--burst] [--out F] | question --id T)\n');
  process.exit(2);
}
