//! Lease distribuido: garantiza que solo un worker sincroniza cada cuenta.
//! Si un worker muere, el lease expira (TTL) y otro toma la cuenta.
//! Compatible con `apps/worker/src/lease.ts` (mismas claves y semántica).

use redis::aio::ConnectionManager;
use redis::Script;
use rvmail_core::events::account_lease_key;
use uuid::Uuid;

const RENEW_LUA: &str = r"
if redis.call('get', KEYS[1]) == ARGV[1] then
  return redis.call('pexpire', KEYS[1], ARGV[2])
else
  return 0
end";

const RELEASE_LUA: &str = r"
if redis.call('get', KEYS[1]) == ARGV[1] then
  return redis.call('del', KEYS[1])
else
  return 0
end";

#[derive(Clone)]
pub struct LeaseManager {
    redis: ConnectionManager,
    owner: String,
    ttl_ms: u64,
}

impl LeaseManager {
    pub fn new(redis: ConnectionManager, owner: String) -> Self {
        Self {
            redis,
            owner,
            ttl_ms: 60_000,
        }
    }

    pub async fn acquire(&self, account: Uuid) -> redis::RedisResult<bool> {
        let mut c = self.redis.clone();
        let r: Option<String> = redis::cmd("SET")
            .arg(account_lease_key(account))
            .arg(&self.owner)
            .arg("PX")
            .arg(self.ttl_ms)
            .arg("NX")
            .query_async(&mut c)
            .await?;
        Ok(r.as_deref() == Some("OK"))
    }

    pub async fn renew(&self, account: Uuid) -> redis::RedisResult<bool> {
        let mut c = self.redis.clone();
        let r: i64 = Script::new(RENEW_LUA)
            .key(account_lease_key(account))
            .arg(&self.owner)
            .arg(self.ttl_ms)
            .invoke_async(&mut c)
            .await?;
        Ok(r == 1)
    }

    pub async fn release(&self, account: Uuid) -> redis::RedisResult<()> {
        let mut c = self.redis.clone();
        let _: i64 = Script::new(RELEASE_LUA)
            .key(account_lease_key(account))
            .arg(&self.owner)
            .invoke_async(&mut c)
            .await?;
        Ok(())
    }
}
