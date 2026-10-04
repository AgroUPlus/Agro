//! Pairing a new device by QR code.

use crate::db::Db;
use async_graphql::{Context, Object, SimpleObject};

use super::{authorize, public_url};

/// Everything a newly created account needs, shown exactly once.
/// A scannable pairing payload, shown once.
#[derive(SimpleObject, Clone)]
pub struct PairingPayload {
    pub qr_data: String,
    pub token: String,
    pub label: String,
}

/// The pairing payload a device scans.
///
/// Carries a freshly minted **device token**, not the account passphrase. The old QR embedded the
/// passphrase, so photographing it once handed over the account permanently rather than one
/// revocable device.
fn pairing_qr(username: &str, device_token: &str) -> String {
    let server = public_url();
    if server.is_empty() {
        format!("agro://connect?username={username}&token={device_token}")
    } else {
        format!(
            "agro://connect?username={}&token={}&server={}",
            username,
            device_token,
            urlencoding::encode(&server)
        )
    }
}

#[derive(Default)]
pub struct PairingMutation;

#[Object]
impl PairingMutation {
    /// Mints a device token and returns it as a scannable pairing payload.
    ///
    /// The pairing QR used to be built from the account passphrase, so photographing it once handed
    /// over the account permanently. Each scan now gets its own revocable credential, which is why
    /// this is a mutation: it creates something.
    async fn pair_device(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        label: Option<String>,
    ) -> async_graphql::Result<PairingPayload> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let label = label
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .ok_or("Name the device, so its token can be told apart from the others")?
            .to_string();
        let token = db.mint_device_token(&user_id, &label)?;
        db.record_event(
            crate::audit::Event::TokenMinted,
            crate::audit::Record::new()
                .user(&user_id)
                .device(label.clone())
                .detail("paired by QR"),
        );
        Ok(PairingPayload {
            qr_data: pairing_qr(&user_id, &token),
            token,
            label,
        })
    }
}
