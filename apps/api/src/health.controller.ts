import { Controller, Get, Inject, Logger, ServiceUnavailableException } from '@nestjs/common';
import { Db } from '@rvmail/db';
import { sql } from 'drizzle-orm';
import Redis from 'ioredis';
import { DB, REDIS } from './infra/infra.module';

@Controller('health')
export class HealthController {
  private readonly log = new Logger('Health');

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
      // El detalle va al log; al cliente no se le exponen datos internos
      this.log.error(`healthcheck falló: ${(e as Error).message}`);
      throw new ServiceUnavailableException({ status: 'error' });
    }
  }
}
