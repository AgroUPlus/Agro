//! Signup preconditions.

use super::*;

/// Bootstrap has to come first, or the instance has no administrator and cannot get one.
///
/// A stranger signing up on an empty database creates account #1, and account #1 existing is
/// exactly what makes `/api/v1/bootstrap` refuse from then on. The result is a server with a
/// pending account and nobody holding the authority to approve it — unrecoverable without
/// touching the database by hand.
#[tokio::test]
async fn nobody_can_sign_up_before_the_server_has_an_administrator() {
    let db = crate::db::Db::new_in_memory().unwrap();
    let state = test_state(db.clone());

    assert_eq!(
        post_signup(state.clone(), "squatter").await,
        StatusCode::UNAUTHORIZED,
        "signup was allowed to claim the empty database"
    );
    assert_eq!(
        db.user_count().unwrap(),
        0,
        "a refused signup still created an account"
    );

    // With an administrator in place it behaves normally again.
    db.create_account("alpha", "alpha-pass", Role::Admin, AccountState::Active)
        .unwrap();
    assert_eq!(
        post_signup(state, "beta").await,
        StatusCode::OK,
        "signup stayed shut after the server had an administrator"
    );
}
