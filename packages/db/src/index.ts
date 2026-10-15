import { drizzle, PostgresJsDatabase } from 'drizzle-orm/postgres-js';
import { migrate } from 'drizzle-orm/postgres-js/migrator';
import path from 'node:path';
import postgres from 'postgres';
import * as schema from './schema';

export * from './schema';
export { schema };

export type Db = PostgresJsDatabase<typeof schema>;

export function createDb(url = process.env.DATABASE_URL, max = 10): { db: Db; sql: postgres.Sql } {
  if (!url) throw new Error('DATABASE_URL no está definida');
  const client = postgres(url, { max, onnotice: () => undefined });
  return { db: drizzle(client, { schema }), sql: client };
}

/** Aplica migraciones pendientes. Seguro de llamar en cada arranque. */
export async function runMigrations(url = process.env.DATABASE_URL): Promise<void> {
  if (!url) throw new Error('DATABASE_URL no está definida');
  const client = postgres(url, { max: 1, onnotice: () => undefined });
  try {
    await migrate(drizzle(client), { migrationsFolder: path.join(__dirname, '..', 'drizzle') });
  } finally {
    await client.end();
  }
}
