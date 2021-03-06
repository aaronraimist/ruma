//! User access tokens.

use base64::encode;
use chrono::{Duration, Utc};
use diesel::pg::data_types::PgTimestamp;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::result::Error as DieselError;
use iron::typemap::Key;
use macaroons::caveat::Caveat;
use macaroons::token::Token;
use macaroons::v1::V1Token;
use ruma_identifiers::UserId;

use crate::error::ApiError;
use crate::schema::access_tokens;

/// A User access token.
#[derive(AsChangeset, Clone, Debug, Identifiable, Queryable)]
#[table_name = "access_tokens"]
pub struct AccessToken {
    /// The access token's ID.
    pub id: i64,
    /// The ID of the user who owns the access token.
    pub user_id: UserId,
    /// The value of the access token. This is a Base64-encoded macaroon.
    pub value: String,
    /// Whether or not the access token has been revoked.
    pub revoked: bool,
    /// The time the access token was created.
    pub created_at: PgTimestamp,
    /// The time the access token was last modified.
    pub updated_at: PgTimestamp,
}

/// A new access token, not yet saved.
#[derive(Debug, Insertable)]
#[table_name = "access_tokens"]
pub struct NewAccessToken {
    /// The ID of the user who owns the access token.
    pub user_id: UserId,
    /// The value of the access token. This is a Base64-encoded macaroon.
    pub value: String,
}

impl AccessToken {
    /// Create a new `AccessToken` for the given user.
    pub fn create(
        connection: &PgConnection,
        user_id: &UserId,
        macaroon_secret_key: &[u8],
    ) -> Result<Self, ApiError> {
        let new_access_token = NewAccessToken {
            user_id: user_id.clone(),
            value: create_macaroon(macaroon_secret_key, user_id)?,
        };

        diesel::insert_into(access_tokens::table)
            .values(&new_access_token)
            .get_result(connection)
            .map_err(ApiError::from)
    }

    /// Creates an `AccessToken` from an access token string value.
    ///
    /// The access token cannot be revoked.
    pub fn find_valid_by_token(
        connection: &PgConnection,
        token: &str,
    ) -> Result<Option<Self>, ApiError> {
        let token = access_tokens::table
            .filter(access_tokens::value.eq(token))
            .filter(access_tokens::revoked.eq(false))
            .first(connection);

        match token {
            Ok(token) => Ok(Some(token)),
            Err(DieselError::NotFound) => Ok(None),
            Err(err) => Err(ApiError::from(err)),
        }
    }

    /// Revoke the access token so it cannot be used again.
    pub fn revoke(&mut self, connection: &PgConnection) -> Result<(), ApiError> {
        self.revoked = true;

        match self.save_changes::<Self>(connection) {
            Ok(_) => Ok(()),
            Err(error) => Err(ApiError::from(error)),
        }
    }
}

impl Key for AccessToken {
    type Value = Self;
}

/// Creates a macaroon for the given user using the master cryptographic key.
fn create_macaroon(macaroon_secret_key: &[u8], user_id: &UserId) -> Result<String, ApiError> {
    let expiration = match Utc::now().checked_add_signed(Duration::hours(1)) {
        Some(datetime) => datetime,
        None => {
            return Err(ApiError::unknown(
                "Failed to generate access token expiration datetime.".to_string(),
            ))
        }
    };

    let token = V1Token::new(macaroon_secret_key, b"key".to_vec(), None)
        .add_caveat(&Caveat::first_party(
            format!("user_id = {}", user_id.to_string())
                .as_bytes()
                .to_owned(),
        ))
        .add_caveat(&Caveat::first_party(b"type = access".to_vec()))
        .add_caveat(&Caveat::first_party(
            format!("time < {}", expiration).as_bytes().to_owned(),
        ));

    let serialized = token.serialize()?;

    Ok(encode(&serialized))
}
