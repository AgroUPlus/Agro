//! TOTP enrolment and recovery codes.

use super::*;
use crate::db::Db;
use crate::schema::{caller, forbidden};
use async_graphql::{Context, Object};

#[derive(Default)]
pub struct SocialSecondFactorMutation;

#[Object]
impl SocialSecondFactorMutation {
    /// Begins enrolling a second factor for the caller.
    ///
    /// Writes a secret that is not yet binding: nothing enforces it until `confirmTotp` has seen a
    /// code computed from it. That ordering is deliberate — the failure mode of a second factor is
    /// always lockout, so the recoverable sequence is the only one on offer.
    async fn begin_totp(&self, ctx: &Context<'_>) -> async_graphql::Result<TotpEnrolmentPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let enrolment = db
            .begin_totp_enrolment(authed.username())
            .map_err(async_graphql::Error::new)?;
        Ok(TotpEnrolmentPayload {
            otpauth_uri: enrolment.otpauth_uri,
            secret_base32: enrolment.secret_base32,
        })
    }

    /// Confirms the pending enrolment with a code from the authenticator.
    ///
    /// **This signs out every other device**, and returns how many. Whoever held the passphrase
    /// before this moment has already traded it for tokens, and those tokens do not carry a second
    /// factor — leaving them alive would make enrolling change nothing for the attacker it is meant
    /// to shut out. The device doing the enrolling is spared, so the user is not signed out of the
    /// screen they are standing on.
    async fn confirm_totp(
        &self,
        ctx: &Context<'_>,
        code: String,
    ) -> async_graphql::Result<TotpConfirmedPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let before = db.list_app_passwords(authed.username())?.len() as i64;
        let spare = Some(authed.token_hash.as_str()).filter(|h| !h.is_empty());
        let recovery_codes = db
            .confirm_totp_enrolment(authed.username(), &code, spare)
            .map_err(async_graphql::Error::new)?;
        let after = db.list_app_passwords(authed.username())?.len() as i64;

        db.record_event(
            crate::audit::Event::TotpEnrolled,
            crate::audit::Record::new()
                .user(authed.username())
                .device(authed.device_label.clone()),
        );
        Ok(TotpConfirmedPayload {
            recovery_codes,
            devices_signed_out: (before - after).max(0),
        })
    }

    /// Turns the second factor off. Requires a current code, or a recovery code.
    ///
    /// The proof is the entire point. Without it, a stolen device token — which by construction did
    /// not pass a second factor — could remove the second factor, and the feature would protect
    /// nothing but itself. An administrator on a server that requires 2FA cannot disable it at all.
    async fn disable_totp(&self, ctx: &Context<'_>, code: String) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        if authed.is_admin() && crate::auth::admin_totp_required() {
            return Err(forbidden(
                "this server requires administrators to keep two-factor authentication enabled",
            ));
        }

        let outcome = db.verify_totp(authed.username(), &code)?;
        if !outcome.is_satisfied() {
            db.record_event(
                crate::audit::Event::TotpFailed,
                crate::audit::Record::new()
                    .user(authed.username())
                    .detail("while trying to disable the second factor"),
            );
            return Err("That code is not right.".into());
        }

        db.disable_totp(authed.username())?;
        db.record_event(
            crate::audit::Event::TotpDisabled,
            crate::audit::Record::new()
                .user(authed.username())
                .device(authed.device_label.clone()),
        );
        Ok(true)
    }

    /// Issues a fresh set of recovery codes, invalidating the old ones. Requires a current code.
    async fn regenerate_recovery_codes(
        &self,
        ctx: &Context<'_>,
        code: String,
    ) -> async_graphql::Result<Vec<String>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let outcome = db.verify_totp(authed.username(), &code)?;
        if !outcome.is_satisfied() {
            return Err("That code is not right.".into());
        }
        let codes = db
            .regenerate_recovery_codes(authed.username())
            .map_err(async_graphql::Error::new)?;
        db.record_event(
            crate::audit::Event::TotpEnrolled,
            crate::audit::Record::new()
                .user(authed.username())
                .detail("recovery codes regenerated"),
        );
        Ok(codes)
    }
}
