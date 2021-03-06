//! Endpoints for logging in users.

use std::convert::TryFrom;
use std::error::Error;
use std::fmt::{Formatter, Result as FmtResult};

use bodyparser;
use iron::{status, Chain, Handler, IronResult, Plugin, Request, Response};
use ruma_identifiers::UserId;
use serde::de::{Deserialize, Deserializer, Error as SerdeError, Visitor};

use crate::authentication::{AuthParams, PasswordAuthParams};
use crate::config::Config;
use crate::db::DB;
use crate::error::ApiError;
use crate::middleware::{JsonRequest, MiddlewareChain};
use crate::models::access_token::AccessToken;
use crate::modifier::SerializableResponse;

/// The `/login` endpoint.
#[derive(Clone, Copy, Debug)]
pub struct Login;

/// The login type specified by the user.
#[derive(Clone, Debug, PartialEq)]
enum LoginType {
    /// The m.login.password type.
    Password,
}

impl<'de> Deserialize<'de> for LoginType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// A serde visitor for deserializing `LoginType`.
        struct LoginTypeVisitor;

        impl<'de> Visitor<'de> for LoginTypeVisitor {
            type Value = LoginType;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> FmtResult {
                write!(formatter, "a login type")
            }

            fn visit_str<E>(self, value: &str) -> Result<LoginType, E>
            where
                E: SerdeError,
            {
                match value {
                    "m.login.password" => Ok(LoginType::Password),
                    _ => Err(SerdeError::custom(
                        "Currenlty only m.login.password is supported",
                    )),
                }
            }
        }

        deserializer.deserialize_any(LoginTypeVisitor)
    }
}

/// The body of the request for this API.
#[derive(Clone, Debug, Deserialize)]
struct LoginRequest {
    /// The login type being used. Currently only "m.login.password" is supported.
    #[serde(rename = "type")]
    pub login_type: LoginType,
    /// The fully qualified user ID or just local part of the user ID, to log in.
    pub user: String,
    /// The user's password.
    pub password: String,
}

/// The body of the response for this API.
#[derive(Debug, Serialize)]
struct LoginResponse {
    /// An access token for the account. This access token can then be used to authorize other requests.
    pub access_token: String,
    /// The hostname of the homeserver on which the account has been registered.
    pub home_server: String,
    /// The fully-qualified Matrix ID that has been registered.
    pub user_id: UserId,
}

middleware_chain!(Login, [JsonRequest]);

impl Handler for Login {
    fn handle(&self, request: &mut Request<'_, '_>) -> IronResult<Response> {
        let login_request = match request.get::<bodyparser::Struct<LoginRequest>>() {
            Ok(Some(request)) => request,
            Ok(None) => Err(ApiError::bad_json(None))?,
            Err(err) => Err(ApiError::bad_json(err.description().to_string()))?,
        };

        let config = Config::from_request(request)?;

        let user_id = match UserId::try_from(login_request.user.as_ref()) {
            Ok(user_id) => {
                if user_id.hostname().to_string() != config.domain {
                    Err(ApiError::unauthorized(
                        "User cannot be identified by this homeserver".to_string(),
                    ))?;
                }

                user_id
            }
            Err(_) => {
                UserId::try_from(format!("@{}:{}", login_request.user, &config.domain).as_ref())
                    .map_err(ApiError::from)?
            }
        };

        let auth_params = AuthParams::Password(PasswordAuthParams {
            password: login_request.password,
            user_id,
        });

        let connection = DB::from_request(request)?;
        let registered_user = auth_params
            .authenticate(&connection)
            .map_err(|_| ApiError::unauthorized("Invalid credentials".to_string()))?;

        let access_token = AccessToken::create(
            &connection,
            &registered_user.id,
            &config.macaroon_secret_key,
        )?;

        let response = LoginResponse {
            access_token: access_token.value,
            home_server: config.domain.clone(),
            user_id: registered_user.id,
        };

        Ok(Response::with((status::Ok, SerializableResponse(response))))
    }
}

#[cfg(test)]
mod tests {
    use crate::test::Test;
    use iron::status::Status;

    #[test]
    fn valid_credentials() {
        let test = Test::new();

        assert!(test
            .register_user(r#"{"username": "carl", "password": "secret"}"#)
            .status
            .is_success());

        let response = test.post(
            "/_matrix/client/r0/login",
            r#"{"type": "m.login.password", "user": "carl", "password": "secret"}"#,
        );

        assert!(response.json().get("access_token").is_some());
        assert_eq!(
            response
                .json()
                .get("home_server")
                .unwrap()
                .as_str()
                .unwrap(),
            "ruma.test"
        );
        assert_eq!(
            response.json().get("user_id").unwrap().as_str().unwrap(),
            "@carl:ruma.test"
        );
    }

    #[test]
    fn invalid_credentials() {
        let test = Test::new();

        let response = test.register_user(r#"{"username": "carl", "password": "secret"}"#);
        assert_eq!(response.status, Status::Ok);

        let response = test.post(
            "/_matrix/client/r0/login",
            r#"{"type": "m.login.password", "user": "carl", "password": "another_secret"}"#,
        );

        assert_eq!(response.status, Status::Forbidden);
    }

    #[test]
    fn invalid_login_type() {
        let test = Test::new();

        let response = test.register_user(r#"{"username": "carl", "password": "secret"}"#);
        assert_eq!(response.status, Status::Ok);

        let response = test.post(
            "/_matrix/client/r0/login",
            r#"{"type": "m.login.email", "user": "carl", "password": "secret"}"#,
        );

        assert_eq!(response.status, Status::UnprocessableEntity);
    }

    #[test]
    fn login_without_register() {
        let test = Test::new();

        let response = test.post(
            "/_matrix/client/r0/login",
            r#"{"type": "m.login.password", "user": "carl", "password": "secret"}"#,
        );

        assert_eq!(response.status, Status::Forbidden);
    }
}
