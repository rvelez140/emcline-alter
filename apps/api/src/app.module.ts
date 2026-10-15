import { Module } from '@nestjs/common';
import { AccountsController } from './accounts/accounts.controller';
import { AuthController } from './auth/auth.controller';
import { EventsController } from './events/events.controller';
import { HealthController } from './health.controller';
import { InfraModule } from './infra/infra.module';
import { MailController } from './mail/mail.controller';
import { MailService } from './mail/mail.service';
import { MailServerAdminController, MailServerPublicController } from './mailserver/mailserver.controller';
import { MailServerService } from './mailserver/mailserver.service';
import { ProvisioningService } from './mailserver/provisioning.service';
import { McpController } from './mcp/mcp.controller';
import { TokensController } from './tokens/tokens.controller';

@Module({
  imports: [InfraModule],
  controllers: [
    HealthController,
    AuthController,
    AccountsController,
    MailController,
    EventsController,
    TokensController,
    McpController,
    MailServerAdminController,
    MailServerPublicController,
  ],
  providers: [MailService, MailServerService, ProvisioningService],
})
export class AppModule {}
