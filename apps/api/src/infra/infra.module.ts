import { Global, Inject, Module, OnApplicationShutdown } from '@nestjs/common';
import { createDb, Db } from '@rvmail/db';
import Redis from 'ioredis';
import { config } from '../config';

export const DB = Symbol('DB');
export const REDIS = Symbol('REDIS');

const { db, sql } = createDb(config.databaseUrl);

@Global()
@Module({
  providers: [
    { provide: DB, useValue: db },
    { provide: REDIS, useFactory: () => new Redis(config.redisUrl, { maxRetriesPerRequest: 3 }) },
  ],
  exports: [DB, REDIS],
})
export class InfraModule implements OnApplicationShutdown {
  constructor(@Inject(REDIS) private readonly redis: Redis) {}

  async onApplicationShutdown() {
    this.redis.disconnect();
    await sql.end({ timeout: 5 });
  }
}

export type { Db };
