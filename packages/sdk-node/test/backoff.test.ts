import assert from 'node:assert/strict';
import { test } from 'node:test';
import { DEFAULT_BACKOFF, computeBackoffDelay } from '../src/backoff.js';

test('the delay grows by the factor from the initial delay', () => {
  const delays = [1, 2, 3, 4].map((attempt) => computeBackoffDelay(attempt, DEFAULT_BACKOFF));
  assert.deepEqual(delays, [100, 200, 400, 800]);
});

test('the delay is capped at maxDelayMs', () => {
  assert.equal(computeBackoffDelay(10, DEFAULT_BACKOFF), 5000);
  assert.equal(computeBackoffDelay(1000, DEFAULT_BACKOFF), 5000);
});

test('a custom factor is honored', () => {
  const options = { initialDelayMs: 10, maxDelayMs: 1000, factor: 3 };
  assert.deepEqual([1, 2, 3].map((attempt) => computeBackoffDelay(attempt, options)), [10, 30, 90]);
});

test('attempt must be an integer of at least 1', () => {
  assert.throws(() => computeBackoffDelay(0, DEFAULT_BACKOFF), RangeError);
  assert.throws(() => computeBackoffDelay(1.5, DEFAULT_BACKOFF), RangeError);
});
