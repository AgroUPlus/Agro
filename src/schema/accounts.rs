//! Accounts as an administrator sees them, and an account's own view of itself.

use crate::auth::AuthedUser;
use crate::db::Db;
use crate::db_identity::AccountState;
use async_graphql::{Context, Object, SimpleObject};

use super::{authorize, caller, forbidden, normalise_username, public_url, require_admin};

#[derive(SimpleObject, Clone)]
/// An account as its owner sees it.
///
/// Carries **no credential**. It used to return `apiKey` and `passphrase` in cleartext, which meant
/// a revocable device token could be traded for the permanent account passphrase just by asking —
/// the escalation that made revoking a device pointless. A credential is shown once, by the
/// mutation that mints it, and never again.
pub struct AccountPayload {
    pub id: String,
    pub username: String,
    pub role: String,
    pub state: String,
    pub quota_bytes: i64,
    pub can_archive: bool,
    pub connection_url: String,
}

fn account_payload(account: &crate::db_identity::Account) -> AccountPayload {
    AccountPayload {
        id: account.id.clone(),
        username: account.username.clone(),
        role: account.role.as_str().to_string(),
        state: account.state.as_str().to_string(),
        quota_bytes: account.quota_bytes,
        can_archive: account.can_archive(),
        connection_url: public_url(),
    }
}

#[derive(Default)]
pub struct AccountsQuery;

#[Object]
impl AccountsQuery {
    /// Every account on the server. Administrators only — this is the guest list, and a guest
    /// enumerating the other guests is the first step of anything else they might try.
    async fn users(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<AccountPayload>> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        Ok(db.list_accounts()?.iter().map(account_payload).collect())
    }

    /// Looks an account up. **Does not create one** — it used to, through `get_or_create_user`,
    /// which made a read-only-looking query mint accounts as a side effect: opening the dashboard
    /// recreated a deleted account, with a new passphrase, and closed the first-run setup window
    /// behind it. Accounts come from `createAccount` and nowhere else.
    /// The caller's own account. `username` is optional and defaults to whoever is asking.
    ///
    /// It has to be optional, because a client's first question is "who am I?" and it cannot name
    /// itself to ask. The dashboard used to guess — it started from a hard-coded `alpha` and asked
    /// `me(username: "alpha")` — so signing in as anyone else produced a refused query, no
    /// correction, and a page that went on displaying somebody else's name.
    async fn me(
        &self,
        ctx: &Context<'_>,
        username: Option<String>,
    ) -> async_graphql::Result<Option<AccountPayload>> {
        let caller = ctx
            .data::<AuthedUser>()
            .map_err(|_| forbidden("Unauthorized"))?;
        let subject = username
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| caller.account.username.clone());
        authorize(ctx, &subject)?;
        let db = ctx.data::<Db>()?;
        Ok(db.account(subject.trim())?.as_ref().map(account_payload))
    }

    /// Everything this server holds about the caller, as a JSON string.
    ///
    /// Self-scoped like everything else — an administrator cannot use this to read someone's
    /// listening history, because `authorize` compares the caller to the named account and an admin
    /// is not exempt from it.
    async fn export_my_data(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<String> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let export = db.export_account_data(&user_id)?;
        Ok(serde_json::to_string_pretty(&export)?)
    }
}

#[derive(Default)]
pub struct AccountsMutation;

#[Object]
impl AccountsMutation {
    /// Approves, suspends or restores an account.
    async fn set_account_state(
        &self,
        ctx: &Context<'_>,
        username: String,
        state: String,
    ) -> async_graphql::Result<AccountPayload> {
        let admin = require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        let target = normalise_username(&username)?;
        let next = AccountState::parse(&state);

        // An admin who suspends themselves locks the deployment out of its own controls, and
        // nothing else can restore them.
        if target.eq_ignore_ascii_case(admin.username()) && !next.is_active() {
            return Err(forbidden(
                "an administrator cannot deactivate their own account",
            ));
        }
        if !db.set_account_state(&target, next)? {
            return Err("No such account".into());
        }
        db.record_event(
            crate::audit::Event::AccountStateChanged,
            crate::audit::Record::new().user(&target).detail(format!(
                "set to {} by {}",
                next.as_str(),
                admin.username()
            )),
        );
        Ok(db
            .account(&target)?
            .as_ref()
            .map(account_payload)
            .expect("just updated"))
    }

    /// Sets how much spool a guest may occupy, in bytes. `0` means unlimited.
    async fn set_account_quota(
        &self,
        ctx: &Context<'_>,
        username: String,
        quota_bytes: i64,
    ) -> async_graphql::Result<AccountPayload> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        let target = normalise_username(&username)?;
        if !db.set_account_quota(&target, quota_bytes)? {
            return Err("No such account".into());
        }
        Ok(db
            .account(&target)?
            .as_ref()
            .map(account_payload)
            .expect("just updated"))
    }

    /// Allows or disallows a user from permanently saving music into the server's archive.
    async fn set_can_archive(
        &self,
        ctx: &Context<'_>,
        username: String,
        can_archive: bool,
    ) -> async_graphql::Result<AccountPayload> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        let target = normalise_username(&username)?;
        if !db.set_can_archive(&target, can_archive)? {
            return Err("No such account".into());
        }
        Ok(db
            .account(&target)?
            .as_ref()
            .map(account_payload)
            .expect("just updated"))
    }

    /// Deletes an account with its nodes, session, settings and app passwords.
    ///
    /// Irreversible, and it can lock you out: deleting the last account puts the server back into
    /// first-run, where anyone who can reach it may create the next one. The caller confirms.
    /// Removes an account and everything belonging to it.
    ///
    /// An administrator may remove anyone; anyone else may only remove themselves. Only the first
    /// half of that used to be true — the check was `authorize`, which compares the caller to the
    /// named account, so the admin could delete nobody but themselves and a guest account could
    /// never be got rid of at all.
    ///
    /// The last administrator is protected. Deleting them would leave a server whose accounts
    /// nobody can administer, and which no setup token can recover: one is only minted for a
    /// database with *no* accounts, and the guests would still be there.
    async fn delete_account(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let target = normalise_username(&username)?;
        let is_self = authed.username().eq_ignore_ascii_case(&target);

        if !is_self && !authed.is_admin() {
            return Err(forbidden(
                "only an administrator can remove another account",
            ));
        }

        let db = ctx.data::<Db>()?;
        let Some(account) = db.account(&target)? else {
            return Ok(false);
        };
        if account.is_admin() && db.admin_count()? <= 1 {
            return Err(forbidden("the last administrator cannot be removed"));
        }

        let removed = db.delete_user(&target)?;
        if removed {
            // Recorded against the *actor*, not the deleted account: rows keyed to a username that
            // no longer exists are exactly what a scoped audit query cannot show anyone, and an
            // account being deleted is something the admin who did it should have to answer for.
            db.record_event(
                crate::audit::Event::AccountDeleted,
                crate::audit::Record::new()
                    .user(authed.username())
                    .detail(format!("removed account {target}")),
            );
        }
        Ok(removed)
    }
}
