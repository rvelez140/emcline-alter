import { accountLeaseKey } from '@rvmail/shared';
import Redis from 'ioredis';

/**
 * Lease distribuido: garantiza que solo un worker sincroniza cada cuenta.
 * Si un worker muere, el lease expira (TTL) y otro toma la cuenta.
 */
const RENEW_LUA = `
if redis.call('get', KEYS[1]) == ARGV[1] then
  return redis.call('pexpire', KEYS[1], ARGV[2])
else
  return 0
end`;

const RELEASE_LUA = `
if redis.call('get', KEYS[1]) == ARGV[1] then
  return redis.call('del', KEYS[1])
else
  return 0
end`;

export class LeaseManager {
  constructor(
    private readonly redis: Redis,
    private readonly owner: string,
    private readonly ttlMs = 60_000,
  ) {}

  async acquire(accountId: string): Promise<boolean> {
    const r = await this.redis.set(accountLeaseKey(accountId), this.owner, 'PX', this.ttlMs, 'NX');
    return r === 'OK';
  }

  async renew(accountId: string): Promise<boolean> {
    const r = await this.redis.eval(RENEW_LUA, 1, accountLeaseKey(accountId), this.owner, String(this.ttlMs));
    return r === 1;
  }

  async release(accountId: string): Promise<void> {
    await this.redis.eval(RELEASE_LUA, 1, accountLeaseKey(accountId), this.owner);
  }
}
