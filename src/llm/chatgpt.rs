// Sign in with ChatGPT: runs Kron's model calls on the user's ChatGPT plan
// instead of an API key, through OpenAI's route for open-source apps that run
// locally (plan.md links its docs). The browser signs in and comes back to a
// listener on 127.0.0.1; OpenAI issues Kron a client ID on the first sign-in
// here; the refresh token is kept in the keyring, and access tokens, an hour
// long, in memory.

use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aws_lc_rs::signature::{RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

use crate::database::schema::device_settings;
use crate::database::{self, Write};

/// What a ChatGPT provider is saved as, in place of a vendor rig knows.
pub const VENDOR: &str = "chatgpt";
/// Where the user sees and limits what apps use of their plan.
pub const USAGE_SETTINGS: &str = "https://chatgpt.com/settings/usage";
/// Where model calls go, with an access token for the key.
pub const API: &str = "https://api.openai.com/v1";

const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const CONFIGURATION: &str = "https://auth.openai.com/.well-known/openid-configuration";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// The scope that lets Kron use the plan; signing in alone doesn't grant it.
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
/// The client ID a first sign-in registers with; never one to keep.
const REGISTER: &str = "dynamic_agent_client";
/// What the user sees Kron called when approving it.
const APP_NAME: &str = "Kron";
const CALLBACK_PATH: &str = "/auth/callback";
/// OpenAI's examples' port; any free one works, so it's tried first.
const PREFERRED_PORT: u16 = 1455;
/// How long the browser has to come back.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait before each retry of a request that never reached OpenAI:
/// a lookup or connection that failed, as a flaky network does for seconds.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];
/// Access tokens are refreshed this long before they expire, so one doesn't
/// lapse during a call.
const EXPIRY_MARGIN: Duration = Duration::from_secs(120);

/// Access tokens by provider. A lock held across a refresh, so two calls
/// don't spend the same rotating refresh token.
static ACCESS: LazyLock<tokio::sync::Mutex<HashMap<Uuid, Access>>> =
    LazyLock::new(Default::default);

/// This computer's registration with OpenAI, from `device_settings`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registration {
    /// `None` until the first sign-in here.
    pub host_id: Option<String>,
    /// What OpenAI issued on the first sign-in; `None` registers anew.
    pub client_id: Option<String>,
}

impl Registration {
    pub fn read(device: &Connection) -> Result<Self, database::Error> {
        Ok(device.query_row(
            "SELECT chatgpt_host_id, chatgpt_client_id FROM device_settings",
            [],
            |row| {
                Ok(Self {
                    host_id: row.get(0)?,
                    client_id: row.get(1)?,
                })
            },
        )?)
    }

    pub fn save(write: &mut Write, host_id: &str, client_id: &str) -> Result<(), database::Error> {
        write.set(device_settings::CHATGPT_HOST_ID, 1, &Some(host_id))?;
        write.set(device_settings::CHATGPT_CLIENT_ID, 1, &Some(client_id))
    }
}

/// What the keyring keeps for a ChatGPT provider, in place of an API key:
/// what refreshing its access needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub client_id: String,
    pub refresh_token: String,
}

impl Credentials {
    pub fn stored(&self) -> String {
        serde_json::json!({
            "client_id": self.client_id,
            "refresh_token": self.refresh_token,
        })
        .to_string()
    }

    pub fn from_stored(stored: &str) -> Result<Self, Error> {
        serde_json::from_str(stored).map_err(|_| Error::SignInAgain)
    }
}

/// A sign-in under way: the browser has `url` to open, and the listener
/// waits for it to come back.
pub struct Attempt {
    pub url: String,
    listener: TcpListener,
    redirect_uri: String,
    state: String,
    nonce: String,
    verifier: String,
    host_id: String,
    /// The client ID this computer was issued before; `None` registers.
    client_id: Option<String>,
}

/// A finished sign-in.
#[derive(Debug)]
pub struct SignedIn {
    pub host_id: String,
    pub credentials: Credentials,
    pub email: Option<String>,
    /// Whether the user let Kron use their plan, not only know who they are.
    pub plan: bool,
    access: Access,
}

#[derive(Clone, Debug)]
struct Access {
    token: String,
    /// When to refresh it.
    renew_at: Instant,
}

#[derive(Debug)]
pub enum Error {
    /// The callback listener couldn't start.
    Listener(std::io::Error),
    Declined,
    TimedOut,
    /// What came back wasn't this attempt's, or wasn't complete.
    Callback(String),
    /// The details are logged where it happened; the user is told only this.
    Network,
    /// OpenAI answered, but not as expected.
    Unreadable,
    /// OpenAI refused a request: its status and error code, when it gave one.
    Refused(u16, Option<String>),
    IdToken(String),
    /// The saved sign-in can't be used; the user signs in again.
    SignInAgain,
}

impl fmt::Display for Error {
    /// What went wrong, for the user.
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Listener(error) => write!(formatter, "Couldn't wait for the browser: {error}."),
            Self::Declined => write!(formatter, "Sign-in was cancelled in the browser."),
            Self::TimedOut => write!(formatter, "Sign-in took too long. Try again."),
            Self::Callback(problem) => write!(formatter, "Sign-in didn't finish: {problem}."),
            Self::Unreadable => write!(formatter, "OpenAI's answer couldn't be read."),
            Self::Network => write!(
                formatter,
                "Couldn't reach OpenAI. Check your internet connection and try again."
            ),
            Self::Refused(status, Some(code)) => {
                write!(formatter, "OpenAI refused the sign-in ({status}, {code}).")
            }
            Self::Refused(status, None) => {
                write!(formatter, "OpenAI refused the sign-in ({status}).")
            }
            Self::IdToken(problem) => {
                write!(formatter, "The sign-in couldn't be verified: {problem}.")
            }
            Self::SignInAgain => write!(formatter, "Sign in with ChatGPT again in Settings."),
        }
    }
}

/// Starts a sign-in: listens on 127.0.0.1 and makes the link for the browser.
pub fn start(registration: &Registration) -> Result<Attempt, Error> {
    let listener = TcpListener::bind(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .map_err(Error::Listener)?;
    let port = listener.local_addr().map_err(Error::Listener)?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let host_id = registration
        .host_id
        .clone()
        .unwrap_or_else(|| format!("urn:uuid:{}", Uuid::from_bytes(random_bytes())));
    let (state, nonce, verifier) = (random(), random(), random());
    let client_id = registration.client_id.clone();
    let mut params = vec![
        ("client_id", client_id.as_deref().unwrap_or(REGISTER)),
        ("ext_agent_host_id", host_id.as_str()),
        ("response_type", "code"),
        ("redirect_uri", redirect_uri.as_str()),
        ("scope", SCOPES),
        ("resource", API),
        ("state", state.as_str()),
        ("nonce", nonce.as_str()),
        ("code_challenge_method", "S256"),
    ];
    let challenge = challenge(&verifier);
    params.push(("code_challenge", challenge.as_str()));
    if client_id.is_none() {
        params.push(("agent_name_hint", APP_NAME));
    }
    let url = Url::parse_with_params(AUTHORIZE, &params)
        .map_err(|error| Error::Callback(error.to_string()))?
        .to_string();
    Ok(Attempt {
        url,
        listener,
        redirect_uri,
        state,
        nonce,
        verifier,
        host_id,
        client_id,
    })
}

/// Waits for the browser, then trades its code for tokens and checks them.
/// Runs on the tokio runtime.
pub async fn finish(attempt: Attempt) -> Result<SignedIn, Error> {
    let Attempt {
        listener,
        redirect_uri,
        state,
        nonce,
        verifier,
        host_id,
        client_id,
        ..
    } = attempt;
    let query = tokio::task::spawn_blocking(move || wait_for_callback(&listener))
        .await
        .map_err(|error| Error::Callback(error.to_string()))??;
    let client_id = callback_client(&query, &state, client_id)?;
    let Some(code) = query.get("code") else {
        return Err(Error::Callback("no code came back".into()));
    };
    let tokens = token_request(&[
        ("grant_type", "authorization_code"),
        ("client_id", &client_id),
        ("code", code),
        ("code_verifier", &verifier),
        ("redirect_uri", &redirect_uri),
        ("resource", API),
    ])
    .await?;
    let Some(id_token) = tokens.id_token.as_deref() else {
        return Err(Error::IdToken("none came back".into()));
    };
    let claims = verify_id_token(id_token, &client_id, &nonce).await?;
    let Some(refresh_token) = tokens.refresh_token.clone() else {
        return Err(Error::Callback("no refresh token came back".into()));
    };
    Ok(SignedIn {
        host_id,
        plan: tokens.grants(PLAN_SCOPE),
        email: claims.email,
        access: tokens.access(),
        credentials: Credentials {
            client_id,
            refresh_token,
        },
    })
}

impl SignedIn {
    pub fn access_token(&self) -> &str {
        &self.access.token
    }
}

/// Keeps a sign-in's access token for `provider`'s calls.
pub async fn remember(provider: Uuid, signed_in: &SignedIn) {
    ACCESS
        .lock()
        .await
        .insert(provider, signed_in.access.clone());
}

/// An access token for `provider`, refreshed when it's near expiry. A
/// refresh rotates the refresh token, which `save` keeps. Runs on the tokio
/// runtime.
pub async fn access_token(
    provider: Uuid,
    credentials: Credentials,
    save: impl FnOnce(&Credentials) -> Result<(), keyring::Error>,
) -> Result<String, Error> {
    let mut access = ACCESS.lock().await;
    if let Some(cached) = access.get(&provider)
        && cached.renew_at > Instant::now()
    {
        return Ok(cached.token.clone());
    }
    let tokens = token_request(&[
        ("grant_type", "refresh_token"),
        ("client_id", &credentials.client_id),
        ("refresh_token", &credentials.refresh_token),
        ("resource", API),
    ])
    .await
    .map_err(|error| match error {
        // An expired, revoked or already used refresh token.
        Error::Refused(400 | 401, _) => Error::SignInAgain,
        error => error,
    })?;
    if let Some(refresh_token) = tokens.refresh_token.clone() {
        let rotated = Credentials {
            refresh_token,
            ..credentials
        };
        if let Err(error) = save(&rotated) {
            // The old one is spent: the next start signs in again.
            eprintln!("chatgpt: couldn't keep the new refresh token: {error}");
        }
    }
    let fresh = tokens.access();
    access.insert(provider, fresh.clone());
    Ok(fresh.token)
}

/// Ends the session at OpenAI and forgets its access token. The client ID
/// stays registered, for the next sign-in here. Runs on the tokio runtime.
pub async fn sign_out(provider: Uuid, credentials: &Credentials) -> Result<(), Error> {
    ACCESS.lock().await.remove(&provider);
    let configuration: Configuration = get_json(CONFIGURATION, None).await?;
    let Some(revocation) = configuration.revocation_endpoint else {
        eprintln!("chatgpt: OpenAI's configuration names no revocation endpoint");
        return Err(Error::Unreadable);
    };
    let response = post_form(
        &revocation,
        &[
            ("token", credentials.refresh_token.as_str()),
            ("token_type_hint", "refresh_token"),
            ("client_id", credentials.client_id.as_str()),
        ],
    )
    .await?;
    match response.status().as_u16() {
        200 => Ok(()),
        status => Err(Error::Refused(status, None)),
    }
}

/// The models the account can use, as (slug, display name), in OpenAI's
/// order. Runs on the tokio runtime.
pub async fn models(access_token: &str) -> Result<Vec<(String, String)>, Error> {
    #[derive(Deserialize)]
    struct Listed {
        models: Vec<Model>,
    }
    #[derive(Deserialize)]
    struct Model {
        slug: String,
        display_name: Option<String>,
        visibility: Option<String>,
    }
    let listed: Listed = get_json(&format!("{API}/models"), Some(access_token)).await?;
    Ok(listed
        .models
        .into_iter()
        .filter(|model| model.visibility.as_deref() == Some("list"))
        .map(|model| {
            let name = model.display_name.unwrap_or_else(|| model.slug.clone());
            (model.slug, name)
        })
        .collect())
}

/// Takes the browser's request to the callback and answers it, ignoring
/// anything else it asks for, like a favicon.
fn wait_for_callback(listener: &TcpListener) -> Result<HashMap<String, String>, Error> {
    listener.set_nonblocking(true).map_err(Error::Listener)?;
    let deadline = Instant::now() + SIGN_IN_TIMEOUT;
    loop {
        if Instant::now() > deadline {
            return Err(Error::TimedOut);
        }
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(error) => return Err(Error::Listener(error)),
        };
        match read_callback(stream) {
            Ok(Some(query)) => return Ok(query),
            Ok(None) => {}
            Err(error) => eprintln!("chatgpt: a request to the callback couldn't be read: {error}"),
        }
    }
}

/// The callback's query, or `None` for a request to anywhere else.
fn read_callback(mut stream: TcpStream) -> std::io::Result<Option<HashMap<String, String>>> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut request_line = String::new();
    BufReader::new(&stream).read_line(&mut request_line)?;
    let target = request_line.split_whitespace().nth(1).unwrap_or_default();
    let query = Url::parse(&format!("http://127.0.0.1{target}"))
        .ok()
        .filter(|url| url.path() == CALLBACK_PATH)
        .map(|url| url.query_pairs().into_owned().collect::<HashMap<_, _>>());
    let (status, body) = match &query {
        Some(query) if query.contains_key("error") => (
            "200 OK",
            "Sign-in was cancelled. You can close this tab and go back to Kron.",
        ),
        Some(_) => (
            "200 OK",
            "Signed in. You can close this tab and go back to Kron.",
        ),
        None => ("404 Not Found", "Not found."),
    };
    let page = format!(
        "<!doctype html><meta charset=utf-8><title>Kron</title>\
         <body style=\"font-family:system-ui;margin:3em\"><p>{body}</p>"
    );
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    )?;
    Ok(query)
}

/// The client ID the callback's code is for, once the callback is checked
/// to be this attempt's.
fn callback_client(
    query: &HashMap<String, String>,
    state: &str,
    saved: Option<String>,
) -> Result<String, Error> {
    if query.get("state").map(String::as_str) != Some(state) {
        return Err(Error::Callback("it wasn't for this sign-in".into()));
    }
    if query.contains_key("error") {
        return Err(Error::Declined);
    }
    let issued = query.get("client_id").cloned();
    match (saved, issued) {
        (Some(saved), Some(issued)) if saved != issued => Err(Error::Callback(
            "it came back for another registration".into(),
        )),
        (Some(saved), _) => Ok(saved),
        (None, Some(issued)) if issued != REGISTER => Ok(issued),
        (None, _) => Err(Error::Callback("Kron wasn't registered".into())),
    }
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
}

impl Tokens {
    fn grants(&self, scope: &str) -> bool {
        self.scope
            .as_deref()
            .is_some_and(|scopes| scopes.split_whitespace().any(|granted| granted == scope))
    }

    fn access(&self) -> Access {
        let lifetime = Duration::from_secs(self.expires_in.unwrap_or(3600));
        Access {
            token: self.access_token.clone(),
            renew_at: Instant::now() + lifetime.saturating_sub(EXPIRY_MARGIN),
        }
    }
}

async fn token_request(form: &[(&str, &str)]) -> Result<Tokens, Error> {
    read_json(post_form(TOKEN, form).await?).await
}

async fn post_form(url: &str, form: &[(&str, &str)]) -> Result<reqwest::Response, Error> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();
    let client = client()?;
    send(|| {
        client
            .post(url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body.clone())
    })
    .await
}

#[derive(Deserialize)]
struct Configuration {
    jwks_uri: String,
    revocation_endpoint: Option<String>,
}

async fn get_json<T: serde::de::DeserializeOwned>(
    url: &str,
    bearer: Option<&str>,
) -> Result<T, Error> {
    let client = client()?;
    let response = send(|| {
        let request = client.get(url);
        match bearer {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    })
    .await?;
    read_json(response).await
}

/// Sends the request, again after a pause when it never reached OpenAI. Even
/// a one-time sign-in code is safe to resend then: OpenAI never saw it. A
/// request that reached it isn't resent.
async fn send(request: impl Fn() -> reqwest::RequestBuilder) -> Result<reqwest::Response, Error> {
    let mut delays = RETRY_DELAYS.iter();
    loop {
        match request().send().await {
            Ok(response) => return Ok(response),
            Err(error) => {
                eprintln!("chatgpt: a request to OpenAI failed: {error}");
                match delays.next() {
                    Some(delay) if error.is_connect() => tokio::time::sleep(*delay).await,
                    _ => return Err(Error::Network),
                }
            }
        }
    }
}

/// The body as `T`, or OpenAI's refusal with its error code. The body of a
/// refusal is left out: it's diagnostic text, not something to show.
async fn read_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, Error> {
    let status = response.status().as_u16();
    let body = response
        .bytes()
        .await
        .map_err(|error| logged(error, Error::Network))?;
    if status != 200 {
        return Err(Error::Refused(status, error_code(&body)));
    }
    serde_json::from_slice(&body).map_err(|error| logged(error, Error::Unreadable))
}

/// The machine-readable code in an OAuth or API error body, or in a
/// `response.failed` event's.
pub fn error_code(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let code = value
        .pointer("/error/code")
        .or_else(|| value.pointer("/response/error/code"))
        .or_else(|| value.get("error"))?;
    code.as_str().map(str::to_owned)
}

fn client() -> Result<reqwest::Client, Error> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| logged(error, Error::Network))
}

/// `error`, for the log, as what the user is told.
fn logged(error: impl fmt::Display, told: Error) -> Error {
    eprintln!("chatgpt: {error}");
    told
}

#[derive(Debug, Deserialize)]
struct Claims {
    iss: String,
    aud: Audience,
    exp: u64,
    nonce: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn includes(&self, client_id: &str) -> bool {
        match self {
            Self::One(audience) => audience == client_id,
            Self::Many(audiences) => audiences.iter().any(|audience| audience == client_id),
        }
    }
}

/// The ID token's claims, once its signature is checked against OpenAI's
/// published keys and the claims against this sign-in.
async fn verify_id_token(id_token: &str, client_id: &str, nonce: &str) -> Result<Claims, Error> {
    #[derive(Deserialize)]
    struct Header {
        alg: String,
        kid: Option<String>,
    }
    #[derive(Deserialize)]
    struct Keys {
        keys: Vec<Key>,
    }
    #[derive(Deserialize)]
    struct Key {
        kid: Option<String>,
        n: Option<String>,
        e: Option<String>,
    }
    let header: Header = decode_part(id_token, 0)?;
    if header.alg != "RS256" {
        return Err(Error::IdToken(format!("it's signed with {}", header.alg)));
    }
    let configuration: Configuration = get_json(CONFIGURATION, None).await?;
    let keys: Keys = get_json(&configuration.jwks_uri, None).await?;
    let key = keys
        .keys
        .into_iter()
        .find(|key| key.kid == header.kid)
        .ok_or_else(|| Error::IdToken("its key isn't OpenAI's".into()))?;
    let (Some(modulus), Some(exponent)) = (key.n, key.e) else {
        return Err(Error::IdToken("OpenAI's key is incomplete".into()));
    };
    verify_signature(id_token, &modulus, &exponent)?;
    let claims: Claims = decode_part(id_token, 1)?;
    check_claims(&claims, client_id, nonce, now())?;
    Ok(claims)
}

/// Checks a JWT's RS256 signature against an RSA key's base64url parts.
fn verify_signature(token: &str, modulus: &str, exponent: &str) -> Result<(), Error> {
    let decode = |part: &str| {
        URL_SAFE_NO_PAD
            .decode(part)
            .map_err(|_| Error::IdToken("it isn't well formed".into()))
    };
    let Some((signed, signature)) = token.rsplit_once('.') else {
        return Err(Error::IdToken("it isn't well formed".into()));
    };
    let key = RsaPublicKeyComponents {
        n: decode(modulus)?,
        e: decode(exponent)?,
    };
    key.verify(
        &RSA_PKCS1_2048_8192_SHA256,
        signed.as_bytes(),
        &decode(signature)?,
    )
    .map_err(|_| Error::IdToken("its signature doesn't match".into()))
}

fn check_claims(claims: &Claims, client_id: &str, nonce: &str, now: u64) -> Result<(), Error> {
    if claims.iss != ISSUER {
        return Err(Error::IdToken("it's from someone else".into()));
    }
    if !claims.aud.includes(client_id) {
        return Err(Error::IdToken("it's for another app".into()));
    }
    if claims.exp <= now {
        return Err(Error::IdToken("it has expired".into()));
    }
    if claims.nonce.as_deref() != Some(nonce) {
        return Err(Error::IdToken("it's from another sign-in".into()));
    }
    Ok(())
}

/// The JSON of a JWT's `index`th part.
fn decode_part<T: serde::de::DeserializeOwned>(token: &str, index: usize) -> Result<T, Error> {
    token
        .split('.')
        .nth(index)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part).ok())
        .and_then(|json| serde_json::from_slice(&json).ok())
        .ok_or_else(|| Error::IdToken("it isn't well formed".into()))
}

/// The PKCE challenge for `verifier`: its SHA-256, base64url without padding.
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// 32 random bytes, base64url: for state, nonce and the PKCE verifier.
fn random() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    // The OS's random source; without it no sign-in is safe, and there is
    // nothing to fall back to.
    getrandom::fill(&mut bytes).expect("the OS has a random source");
    bytes
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::rsa::{KeyPair, KeySize};
    use aws_lc_rs::signature::{KeyPair as _, RSA_PKCS1_SHA256};
    use std::io::Read as _;

    /// An RSA public key's modulus and exponent, from its PKCS#1 DER:
    /// SEQUENCE { INTEGER n, INTEGER e }.
    fn modulus_and_exponent(der: &[u8]) -> (Vec<u8>, Vec<u8>) {
        fn element(der: &[u8]) -> (&[u8], &[u8]) {
            let (length, header) = match der[1] {
                short if short < 0x80 => (short as usize, 2),
                long => {
                    let count = (long & 0x7f) as usize;
                    let length = der[2..2 + count]
                        .iter()
                        .fold(0, |length, byte| length << 8 | *byte as usize);
                    (length, 2 + count)
                }
            };
            (&der[header..header + length], &der[header + length..])
        }
        let (sequence, _) = element(der);
        let (modulus, rest) = element(sequence);
        let (exponent, _) = element(rest);
        let unsigned = |integer: &[u8]| {
            integer
                .iter()
                .skip_while(|byte| **byte == 0)
                .copied()
                .collect::<Vec<u8>>()
        };
        (unsigned(modulus), unsigned(exponent))
    }

    #[test]
    fn the_challenge_is_the_verifiers_sha256_in_base64url() {
        // SHA-256("abc"), a FIPS 180-2 test vector.
        assert_eq!(
            challenge("abc"),
            "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0"
        );
        assert_eq!(random().len(), 43, "32 bytes, a valid PKCE verifier");
    }

    #[test]
    fn a_first_sign_in_registers_and_names_kron() {
        let attempt = match start(&Registration::default()) {
            Ok(attempt) => attempt,
            Err(error) => panic!("{error}"),
        };
        let url = match Url::parse(&attempt.url) {
            Ok(url) => url,
            Err(error) => panic!("{error}"),
        };
        let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(params.get("client_id").map(String::as_str), Some(REGISTER));
        assert_eq!(
            params.get("agent_name_hint").map(String::as_str),
            Some(APP_NAME)
        );
        assert!(params["ext_agent_host_id"].starts_with("urn:uuid:"));
        assert!(params["redirect_uri"].starts_with("http://127.0.0.1:"));
        assert!(params["redirect_uri"].ends_with(CALLBACK_PATH));
        assert_eq!(params["code_challenge"], challenge(&attempt.verifier));
        assert_eq!(params["scope"], SCOPES);
        assert_eq!(params["resource"], API);
    }

    #[test]
    fn a_later_sign_in_reuses_the_issued_client_without_registering() {
        let registration = Registration {
            host_id: Some("urn:uuid:00000000-0000-4000-8000-000000000000".into()),
            client_id: Some("oaiapp_kron".into()),
        };
        let attempt = match start(&registration) {
            Ok(attempt) => attempt,
            Err(error) => panic!("{error}"),
        };
        let params: HashMap<String, String> = match Url::parse(&attempt.url) {
            Ok(url) => url.query_pairs().into_owned().collect(),
            Err(error) => panic!("{error}"),
        };
        assert_eq!(params["client_id"], "oaiapp_kron");
        assert_eq!(
            params["ext_agent_host_id"],
            registration.host_id.unwrap_or_default()
        );
        assert!(!params.contains_key("agent_name_hint"));
    }

    #[test]
    fn the_listener_answers_the_callback_and_ignores_other_requests() {
        let attempt = match start(&Registration::default()) {
            Ok(attempt) => attempt,
            Err(error) => panic!("{error}"),
        };
        let address = match attempt.listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("{error}"),
        };
        let browser = std::thread::spawn(move || {
            let get = |target: &str| -> std::io::Result<String> {
                let mut stream = TcpStream::connect(address)?;
                write!(stream, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")?;
                let mut answer = String::new();
                std::io::Read::read_to_string(&mut stream, &mut answer)?;
                Ok(answer)
            };
            (
                get("/favicon.ico"),
                get("/auth/callback?code=the-code&state=the-state&client_id=oaiapp_new"),
            )
        });
        let query = wait_for_callback(&attempt.listener);
        let (favicon, callback) = match browser.join() {
            Ok(answers) => answers,
            Err(_) => panic!("the browser thread panicked"),
        };
        assert!(favicon.is_ok_and(|answer| answer.starts_with("HTTP/1.1 404")));
        assert!(callback.is_ok_and(|answer| answer.contains("Signed in.")));
        let query = match query {
            Ok(query) => query,
            Err(error) => panic!("{error}"),
        };
        assert_eq!(query["code"], "the-code");
        assert_eq!(
            callback_client(&query, "the-state", None).ok().as_deref(),
            Some("oaiapp_new")
        );
    }

    #[test]
    fn a_callback_from_another_attempt_or_registration_is_refused() {
        let query = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect()
        };
        let issued = query(&[("state", "s"), ("code", "c"), ("client_id", "oaiapp_a")]);
        assert!(matches!(
            callback_client(&issued, "other", None),
            Err(Error::Callback(_))
        ));
        assert!(matches!(
            callback_client(&issued, "s", Some("oaiapp_b".into())),
            Err(Error::Callback(_))
        ));
        let declined = query(&[("state", "s"), ("error", "access_denied")]);
        assert!(matches!(
            callback_client(&declined, "s", None),
            Err(Error::Declined)
        ));
        let unregistered = query(&[("state", "s"), ("code", "c")]);
        assert!(matches!(
            callback_client(&unregistered, "s", None),
            Err(Error::Callback(_))
        ));
        assert_eq!(
            callback_client(&unregistered, "s", Some("oaiapp_a".into()))
                .ok()
                .as_deref(),
            Some("oaiapp_a"),
            "a returning sign-in may leave the client out"
        );
    }

    #[test]
    fn an_id_token_is_checked_against_its_key_and_this_sign_in() {
        let key = match KeyPair::generate(KeySize::Rsa2048) {
            Ok(key) => key,
            Err(error) => panic!("{error}"),
        };
        let (modulus, exponent) = modulus_and_exponent(key.public_key().as_ref());
        let (modulus, exponent) = (
            URL_SAFE_NO_PAD.encode(modulus),
            URL_SAFE_NO_PAD.encode(exponent),
        );
        let sign = |claims: serde_json::Value| -> String {
            let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","kid":"k"}"#);
            let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
            let signed = format!("{header}.{payload}");
            let mut signature = vec![0; key.public_modulus_len()];
            if let Err(error) = key.sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                signed.as_bytes(),
                &mut signature,
            ) {
                panic!("{error}");
            }
            format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature))
        };
        let claims = serde_json::json!({
            "iss": ISSUER, "aud": ["oaiapp_kron"], "exp": 2_000_000_000u64,
            "nonce": "n", "email": "user@example.com", "sub": "user",
        });
        let token = sign(claims.clone());
        assert!(verify_signature(&token, &modulus, &exponent).is_ok());
        let mut tampered = token.clone();
        tampered.insert(token.find('.').map_or(0, |dot| dot + 2), 'x');
        assert!(verify_signature(&tampered, &modulus, &exponent).is_err());

        let read: Claims = match decode_part(&token, 1) {
            Ok(claims) => claims,
            Err(error) => panic!("{error}"),
        };
        assert_eq!(read.email.as_deref(), Some("user@example.com"));
        let now = 1_900_000_000;
        assert!(check_claims(&read, "oaiapp_kron", "n", now).is_ok());
        assert!(check_claims(&read, "oaiapp_other", "n", now).is_err());
        assert!(check_claims(&read, "oaiapp_kron", "other", now).is_err());
        assert!(check_claims(&read, "oaiapp_kron", "n", 2_100_000_000).is_err());
    }

    #[test]
    fn the_registration_is_kept_on_this_device() -> Result<(), database::Error> {
        let mut writer = crate::database::Writer::open_in_memory()?;
        assert_eq!(
            Registration::read(writer.device())?,
            Registration::default()
        );
        writer.execute(|write| Registration::save(write, "urn:uuid:host", "oaiapp_kron"))?;
        assert_eq!(
            Registration::read(writer.device())?,
            Registration {
                host_id: Some("urn:uuid:host".into()),
                client_id: Some("oaiapp_kron".into()),
            }
        );
        Ok(())
    }

    #[test]
    fn credentials_round_trip_through_the_keyring_text() {
        let credentials = Credentials {
            client_id: "oaiapp_kron".into(),
            refresh_token: "refresh".into(),
        };
        assert_eq!(
            Credentials::from_stored(&credentials.stored()).ok(),
            Some(credentials)
        );
        assert!(matches!(
            Credentials::from_stored("an API key"),
            Err(Error::SignInAgain)
        ));
    }

    #[test]
    fn an_error_bodys_code_is_read_from_either_shape() {
        assert_eq!(
            error_code(br#"{"error":{"code":"subscription_sharing_usage_limit_exceeded"}}"#)
                .as_deref(),
            Some("subscription_sharing_usage_limit_exceeded")
        );
        assert_eq!(
            error_code(br#"{"error":"invalid_grant"}"#).as_deref(),
            Some("invalid_grant")
        );
        assert_eq!(
            error_code(br#"{"type":"response.failed","response":{"error":{"code":"x"}}}"#)
                .as_deref(),
            Some("x")
        );
        assert_eq!(error_code(br#"{"detail":"no"}"#), None);
    }

    /// A sign-in that came back while the network was down for a moment
    /// failed at once, with the token endpoint's URL as its message.
    #[test]
    fn a_request_that_never_reached_openai_is_sent_again() -> Result<(), Box<dyn std::error::Error>>
    {
        let port = std::net::TcpListener::bind("127.0.0.1:0")?
            .local_addr()?
            .port();
        let server = std::thread::spawn(move || -> std::io::Result<()> {
            std::thread::sleep(Duration::from_millis(300));
            let listener = std::net::TcpListener::bind(("127.0.0.1", port))?;
            let (mut stream, _) = listener.accept()?;
            let mut request = [0; 1024];
            let _read = stream.read(&mut request)?;
            stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            )
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let answer: serde_json::Value = runtime
            .block_on(get_json(&format!("http://127.0.0.1:{port}/"), None))
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(answer, serde_json::json!({}));
        server.join().map_err(|_| "the server panicked")??;
        Ok(())
    }

    #[test]
    fn a_network_failure_says_so_without_the_details() {
        let message = logged(
            "error sending request for url (https://auth.openai.com/api/accounts/oauth/token)",
            Error::Network,
        )
        .to_string();
        assert_eq!(
            message,
            "Couldn't reach OpenAI. Check your internet connection and try again."
        );
    }
}
