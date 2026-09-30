import { randomBytes } from 'node:crypto';

const CROCKFORD = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';

/** 48bit ms 時刻 (10 文字) + 80bit 乱数 (16 文字)。辞書順 = 時刻順。同一 ms 内は単調増加させる。 */
let lastTime = -1;
let lastRandom: bigint = 0n;

export function ulid(now: number = Date.now()): string {
  let random: bigint;
  if (now <= lastTime) {
    now = lastTime;
    random = (lastRandom + 1n) & ((1n << 80n) - 1n);
  } else {
    random = BigInt('0x' + randomBytes(10).toString('hex'));
  }
  lastTime = now;
  lastRandom = random;
  return encode(BigInt(now), 10) + encode(random, 16);
}

function encode(value: bigint, length: number): string {
  let out = '';
  for (let i = 0; i < length; i++) {
    out = CROCKFORD[Number(value & 31n)] + out;
    value >>= 5n;
  }
  return out;
}

export function newPaneId(): string {
  return `p_${ulid()}`;
}
