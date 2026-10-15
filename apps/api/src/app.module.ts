import { Module } from '@nestjs/common';
import { AccountsController } from './accounts/accounts.controller';
import { AuthController } from './auth/auth.controller';
import { EventsController } from './events/events.controller';
import { HealthController } from './health.controller';
import { InfraModule } from './infra/infra.module';
import { MailController } from './mail/mail.controller';

@Module({
  imports: [InfraModule],
  controllers: [HealthController, AuthController, AccountsController, MailController, EventsController],
})
export class AppModule {}
