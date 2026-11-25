//! The login flows, driven step by step against [`FakeApi`](crate::testing::FakeApi).
//!
//! These pin the step ids the app shows and the order of the calls a login makes. The bot login's
//! cloud-API logout is the one thing here that needs the network, so only its first step is tested.

use std::collections::BTreeMap;

use chirper_bridge::{Error, LoginCtx, LoginInput, LoginProcess, LoginStep};

use crate::Credentials;
use crate::error::CallError;
use crate::login::{
    BotLogin, PhoneLogin, QrLogin, STEP_BOT_TOKEN, STEP_CODE, STEP_CODE_INCORRECT, STEP_PASSWORD,
    STEP_PHONE_NUMBER, STEP_SHOW_QR,
};
use crate::testing::{self, FakeApi};
use crate::tl::{Identifiable, enums, functions, types};

/// The application credentials a test login uses.
fn credentials() -> Credentials {
    Credentials {
        api_id: 1,
        api_hash: "hash".to_owned(),
    }
}

/// A login input with the one field a step asked for.
fn input(field: &str, value: &str) -> LoginInput {
    LoginInput::values(BTreeMap::from([(field.to_owned(), value.to_owned())]))
}

/// The step id of a step that asks for values, or the name of the step kind.
fn step_id(step: &LoginStep) -> String {
    match step {
        LoginStep::Complete { .. } => "complete".to_owned(),
        other => other.step_id().to_owned(),
    }
}

fn phone_login(api: std::sync::Arc<FakeApi>) -> PhoneLogin<std::sync::Arc<FakeApi>> {
    PhoneLogin::new(api, testing::ctx("phone"), credentials())
}

#[tokio::test]
async fn the_phone_login_asks_for_the_number_the_code_and_finishes() {
    let api = FakeApi::new();
    api.answer_to(
        functions::auth::SendCode::CONSTRUCTOR_ID,
        &testing::sent_code_sms("h1"),
    );
    api.answer_to(
        functions::auth::SignIn::CONSTRUCTOR_ID,
        &testing::authorization(7),
    );
    let mut login = phone_login(std::sync::Arc::clone(&api));

    let first = login
        .next(LoginInput::start())
        .await
        .expect("the first step");
    assert_eq!(step_id(&first), STEP_PHONE_NUMBER);

    let second = login
        .next(input(STEP_PHONE_NUMBER, " +15550009 "))
        .await
        .expect("the code step");
    assert_eq!(step_id(&second), STEP_CODE);
    let LoginStep::UserInput { fields, .. } = &second else {
        panic!("the code step asks for values");
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].kind, "number");

    let third = login
        .next(input(STEP_CODE, "12345"))
        .await
        .expect("the end");
    let LoginStep::Complete { login_id } = third else {
        panic!("the login finished");
    };
    assert_eq!(login_id, "7");

    // The phone number went out trimmed, with the hash the server handed back, and the code.
    let sign_in = api
        .seen_with(functions::auth::SignIn::CONSTRUCTOR_ID)
        .pop()
        .expect("one sign-in");
    assert!(sign_in.carries("+15550009"));
    assert!(sign_in.carries("h1"));
    assert!(sign_in.carries("12345"));

    // The application credentials went out with the code request.
    let send_code = api
        .seen_with(functions::auth::SendCode::CONSTRUCTOR_ID)
        .pop()
        .expect("one code request");
    assert!(send_code.carries("hash"));
}

#[tokio::test]
async fn a_wrong_code_asks_for_it_again() {
    let api = FakeApi::new();
    api.answer_to(
        functions::auth::SendCode::CONSTRUCTOR_ID,
        &testing::sent_code_sms("h1"),
    );
    api.fail(
        &functions::auth::SignIn {
            phone_number: String::new(),
            phone_code_hash: String::new(),
            phone_code: None,
            email_verification: None,
        },
        CallError::Rpc {
            code: 400,
            name: "PHONE_CODE_INVALID".to_owned(),
        },
    );
    let mut login = phone_login(api);
    login
        .next(input(STEP_PHONE_NUMBER, "+15550009"))
        .await
        .expect("the code step");
    let step = login.next(input(STEP_CODE, "00000")).await.expect("a step");
    assert_eq!(step_id(&step), STEP_CODE_INCORRECT);
}

#[tokio::test]
async fn two_step_verification_asks_for_the_password() {
    let api = FakeApi::new();
    api.answer_to(
        functions::auth::SendCode::CONSTRUCTOR_ID,
        &testing::sent_code_sms("h1"),
    );
    api.fail(
        &functions::auth::SignIn {
            phone_number: String::new(),
            phone_code_hash: String::new(),
            phone_code: None,
            email_verification: None,
        },
        CallError::Rpc {
            code: 401,
            name: "SESSION_PASSWORD_NEEDED".to_owned(),
        },
    );
    let mut login = phone_login(api);
    login
        .next(input(STEP_PHONE_NUMBER, "+15550009"))
        .await
        .expect("the code step");
    let step = login.next(input(STEP_CODE, "12345")).await.expect("a step");
    assert_eq!(step_id(&step), STEP_PASSWORD);
}

#[tokio::test]
async fn a_banned_number_is_explained_and_no_auth_key_is_kept() {
    let api = FakeApi::new();
    api.fail(
        &functions::auth::SendCode {
            phone_number: String::new(),
            api_id: 1,
            api_hash: String::new(),
            settings: Box::new(enums::CodeSettings::CodeSettings(types::CodeSettings {
                allow_flashcall: false,
                current_number: false,
                allow_app_hash: false,
                allow_missed_call: false,
                allow_firebase: false,
                unknown_number: false,
                logout_tokens: None,
                token: None,
                app_sandbox: None,
            })),
        },
        CallError::Rpc {
            code: 400,
            name: "PHONE_NUMBER_BANNED".to_owned(),
        },
    );
    let mut login = phone_login(api);
    let error = login
        .next(input(STEP_PHONE_NUMBER, "+15550009"))
        .await
        .expect_err("a refusal");
    assert!(matches!(error, Error::Protocol(ref text) if text.contains("banned")));
}

#[tokio::test]
async fn the_qr_login_shows_a_token_to_scan() {
    let api = FakeApi::new();
    api.answer_to(
        functions::auth::ExportLoginToken::CONSTRUCTOR_ID,
        &enums::auth::LoginToken::LoginToken(types::auth::LoginToken {
            expires: 1_700_000_060,
            token: b"token".to_vec(),
        }),
    );
    let (_frames_tx, frames) = tokio::sync::mpsc::unbounded_channel();
    let mut login = QrLogin::new(api, testing::ctx("qr"), credentials(), frames);
    let step = login.next(LoginInput::start()).await.expect("the QR step");
    let LoginStep::DisplayAndWait {
        step_id: id,
        kind,
        data,
        ..
    } = &step
    else {
        panic!("the QR login shows something to scan");
    };
    assert_eq!(id, STEP_SHOW_QR);
    assert_eq!(kind, "qr");
    assert!(data.starts_with("tg://login?token="), "{data}");
}

#[tokio::test]
async fn the_bot_login_asks_for_the_token() {
    let api = FakeApi::new();
    let mut login = BotLogin::new(api, testing::ctx("bot"), credentials());
    let step = login
        .next(LoginInput::start())
        .await
        .expect("the token step");
    assert_eq!(step_id(&step), STEP_BOT_TOKEN);
    let empty = login
        .next(input(STEP_BOT_TOKEN, "  "))
        .await
        .expect("again");
    assert_eq!(step_id(&empty), STEP_BOT_TOKEN);
}

#[tokio::test]
async fn the_code_step_explains_where_the_code_went() {
    // `SentCodeTypeApp` is the default in the reference and the most common in practice.
    let api = FakeApi::new();
    api.answer_to(
        functions::auth::SendCode::CONSTRUCTOR_ID,
        &enums::auth::SentCode::SentCode(types::auth::SentCode {
            r#type: Box::new(enums::auth::SentCodeType::SentCodeTypeApp(
                types::auth::SentCodeTypeApp { length: 5 },
            )),
            phone_code_hash: "h2".to_owned(),
            next_type: None,
            timeout: None,
        }),
    );
    let mut login = phone_login(api);
    let step = login
        .next(input(STEP_PHONE_NUMBER, "+15550009"))
        .await
        .expect("the code step");
    let LoginStep::UserInput { fields, .. } = &step else {
        panic!("the code step asks for values");
    };
    assert!(
        fields[0].description.contains("Telegram app"),
        "{}",
        fields[0].description
    );
}

/// The fake must report a request it has no answer for instead of hanging.
#[tokio::test]
async fn an_unanswered_call_is_a_protocol_error() {
    use crate::client::Api;
    let api = FakeApi::new();
    let error = api
        .call(&functions::updates::GetState {})
        .await
        .expect_err("no answer queued");
    assert!(matches!(error, CallError::Protocol(_)));
    let seen = api.seen_with(functions::updates::GetState::CONSTRUCTOR_ID);
    assert_eq!(seen.len(), 1);
}

/// The credentials type must not print the API hash.
#[test]
fn credentials_are_redacted() {
    let text = format!("{:?}", credentials());
    assert!(!text.contains("\"hash\""), "{text}");
    assert!(text.contains("[redacted]"), "{text}");
}

/// The context helper must not reuse a directory between tests.
#[test]
fn test_contexts_get_their_own_directory() {
    let first: LoginCtx = testing::ctx("same");
    let second = testing::ctx("same");
    assert_ne!(first.data_dir, second.data_dir);
}
