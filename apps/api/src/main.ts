import 'reflect-metadata';
import { Logger } from '@nestjs/common';
import { NestFactory } from '@nestjs/core';
import { NestExpressApplication } from '@nestjs/platform-express';
import { runMigrations } from '@rvmail/db';
import cookieParser from 'cookie-parser';
import { AppModule } from './app.module';
import { config } from './config';

async function bootstrap() {
  const log = new Logger('RVMail');
  if (config.runMigrations) {
    log.log('Aplicando migraciones…');
    await runMigrations(config.databaseUrl);
  }

  const app = await NestFactory.create<NestExpressApplication>(AppModule, { bodyParser: true });
  app.set('trust proxy', 1);
  app.useBodyParser('json', { limit: '25mb' });
  app.use(cookieParser());
  app.setGlobalPrefix('api');
  app.enableShutdownHooks();

  await app.listen(config.port, '0.0.0.0');
  log.log(`API escuchando en :${config.port}`);
}

bootstrap().catch((err) => {
  console.error('Error fatal al iniciar la API:', err);
  process.exit(1);
});
