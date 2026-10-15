import { Controller, Get, Inject, ServiceUnavailableException } from '@nestjs/common';
import { Db } from '@rvmail/db';
import { sql } from 'drizzle-orm';
import Redis from 'ioredis';
import { DB, REDIS } from './infra/infra.module';

@Controller('health')
export class HealthController {
  constructor(
    @Inject(DB) private readonly db: Db,
    @Inject(REDIS) private readonly redis: Redis,
  ) {}

  @Get()
  async check() {
    try {
      await this.db.execute(sql`select 1`);
      await this.redis.ping();
      return { status: 'ok', version: process.env.APP_VERSION ?? 'dev' };
    } catch (e) {
      throw new ServiceUnavailableException({ status: 'error', error: (e as Error).message });
    }
  }
}
