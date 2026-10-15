function required(name: string): string {
  const v = process.env[name];
  if (!v) throw new Error(`Falta la variable de entorno ${name}`);
  return v;
}

export const config = {
  port: Number(process.env.API_PORT ?? 3000),
  databaseUrl: required('DATABASE_URL'),
  redisUrl: process.env.REDIS_URL ?? 'redis://localhost:6379',
  jwtSecret: required('JWT_SECRET'),
  /** MASTER_KEY se valida al cifrar/descifrar (packages/shared). */
  masterKey: required('MASTER_KEY'),
  allowSignup: process.env.ALLOW_SIGNUP === 'true',
  runMigrations: process.env.RUN_MIGRATIONS !== 'false',
  secureCookies: process.env.NODE_ENV === 'production' && process.env.INSECURE_COOKIES !== 'true',
  sessionDays: Number(process.env.SESSION_DAYS ?? 30),
  appUrl: process.env.APP_URL ?? 'http://localhost:5173',
};
