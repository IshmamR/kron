// Calls to AI models. The provider and the model each task runs on are per
// device (`llm_providers`, `llm_assignments` in device.sqlite); a provider's
// API key is in the OS keyring, named by its id, never in the database.
// Calls go through rig, on a tokio runtime of their own, since rig's HTTP
// client needs one and GPUI doesn't run one. Nothing here logs what's sent or
// answered.

pub mod chatgpt;

use std::fmt;
use std::time::Duration;

use gpui_kit::{App, AppContext, Global, Task};
use rig_core::ProviderError;
use rig_core::catalog::Catalog;
use rig_core::completion::{CompletionRequest, CompletionResponse, Effort, Reasoning};
use rig_core::driver::DynModel;
use rig_core::operation::Completion;
use rig_core::providers::openai::wire::{Auth, ResponsesContract};
use rig_core::providers::openai::{OpenAIConfig, Route};
use rig_core::providers::registry::{ProviderConfig, ProviderId};
use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use crate::database::schema::{
    LlmAssignments, LlmCalls, LlmProviders, LlmRuns, llm_assignments, llm_calls, llm_providers,
    llm_runs,
};
use crate::database::{self, Write};

/// The keyring service keys are kept under, each named by its provider's id.
const KEYRING_SERVICE: &str = "Kron";
/// The vendors Kron offers, popular ones only: any other, or a model on the
/// user's own machine, is reached as an OpenAI-compatible server.
pub const VENDORS: &[Vendor] = &[
    Vendor {
        id: "anthropic",
        name: "Anthropic",
        keys_at: "console.anthropic.com",
        model: "claude-haiku-4-5",
    },
    Vendor {
        id: "openai",
        name: "OpenAI",
        keys_at: "platform.openai.com",
        model: "gpt-5.4-mini",
    },
    Vendor {
        id: "gcp.gemini",
        name: "Google Gemini",
        keys_at: "aistudio.google.com",
        model: "gemini-flash-latest",
    },
    Vendor {
        id: "deepseek",
        name: "DeepSeek",
        keys_at: "platform.deepseek.com",
        model: "deepseek-flash",
    },
    Vendor {
        id: "xai",
        name: "xAI",
        keys_at: "console.x.ai",
        model: "grok-4.20-0309-non-reasoning",
    },
    Vendor {
        id: "mistral",
        name: "Mistral",
        keys_at: "console.mistral.ai",
        model: "mistral-medium-latest",
    },
    Vendor {
        id: "groq",
        name: "Groq",
        keys_at: "console.groq.com",
        model: "openai/gpt-oss-120b",
    },
    Vendor {
        id: "openrouter",
        name: "OpenRouter",
        keys_at: "openrouter.ai",
        model: "deepseek/deepseek-chat",
    },
];
/// The vendor an OpenAI-compatible server is saved under, with its own URL.
const COMPATIBLE_VENDOR: &str = "openai";
/// Its `format`: OpenAI's Chat Completions, which such servers serve, not
/// the Responses endpoint OpenAI's own defaults to.
const CHAT_FORMAT: &str = "chat";
/// Ours: a CV's facts come back in well under this; past it, the call has
/// hung.
const TIMEOUT: Duration = Duration::from_secs(180);
/// A server lists its models at once, or it isn't there.
const LISTING_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait before each retry of a request that never reached the
/// provider: a lookup or connection that failed, as a flaky network does for
/// seconds.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

pub fn init(cx: &mut App) {
    match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("llm")
        .enable_all()
        .build()
    {
        Ok(runtime) => cx.set_global(Runtime(runtime)),
        Err(error) => eprintln!("llm: couldn't start the runtime for calls: {error}"),
    }
}

struct Runtime(tokio::runtime::Runtime);

impl Global for Runtime {}

/// What a model is asked to do, as `llm_assignments` and `llm_runs` name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    ExtractFacts,
    GenerateResume,
}

impl Purpose {
    pub fn stored(self) -> &'static str {
        match self {
            Self::ExtractFacts => "extract_facts",
            Self::GenerateResume => "generate_resume",
        }
    }
}

/// A vendor Kron offers, by rig's vendor id.
pub struct Vendor {
    pub id: &'static str,
    pub name: &'static str,
    /// Where the user makes a key.
    pub keys_at: &'static str,
    /// The model a first provider runs everything on, until the user picks
    /// another: a fast, cheap one, since copying facts out needs no more.
    pub model: &'static str,
}

/// A provider saved on this device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provider {
    pub id: Uuid,
    pub name: String,
    pub vendor: String,
    /// `CHAT_FORMAT` for an OpenAI-compatible server; `None`: the vendor's.
    pub format: Option<String>,
    /// `None`: the vendor's own endpoint.
    pub base_url: Option<String>,
}

impl Provider {
    /// A server the user added by its URL, rather than a vendor's own API.
    /// Its key is optional: a model on the user's machine takes none.
    pub fn is_compatible(&self) -> bool {
        self.base_url.is_some()
    }

    /// Signed in with ChatGPT: runs on the user's plan, with no API key.
    pub fn is_chatgpt(&self) -> bool {
        self.vendor == chatgpt::VENDOR
    }
}

/// A provider, with the model a task runs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assigned {
    pub provider: Provider,
    pub model: String,
}

/// An answer, with what it cost.
pub struct Reply {
    pub response: CompletionResponse,
    /// In USD: the provider's figure, or the catalog's list price of the
    /// tokens; `None` when neither is known.
    pub cost: Option<f64>,
}

/// A task run, to record in `llm_runs`, with each call it made in
/// `llm_calls`.
pub struct Run<'a> {
    pub purpose: Purpose,
    pub assigned: &'a Assigned,
    /// In ms since the Unix epoch.
    pub started_at: i64,
    pub duration: Duration,
    /// In the order they were made.
    pub calls: &'a [Call<'a>],
    /// Whether the answer was used: one that couldn't be read wasn't.
    pub done: bool,
    /// What it ran for: a document's facts, or a job's résumé, and the
    /// résumé made.
    pub document: Option<Uuid>,
    pub job: Option<Uuid>,
    pub generation: Option<Uuid>,
}

/// One call of a run.
pub struct Call<'a> {
    pub step: Step,
    pub duration: Duration,
    /// The answer, if one came.
    pub reply: Option<&'a Reply>,
}

/// What a call of a run was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    ExtractFacts,
    Write,
    CheckMeaning,
}

impl Step {
    fn stored(self) -> &'static str {
        match self {
            Self::ExtractFacts => "extract_facts",
            Self::Write => "write",
            Self::CheckMeaning => "check_meaning",
        }
    }

    fn from_stored(stored: &str) -> Option<Self> {
        match stored {
            "extract_facts" => Some(Self::ExtractFacts),
            "write" => Some(Self::Write),
            "check_meaning" => Some(Self::CheckMeaning),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ExtractFacts => "Finding facts",
            Self::Write => "Writing",
            Self::CheckMeaning => "Checking claims",
        }
    }
}

/// What the calls that wrote a résumé took and cost.
#[derive(Clone, Debug, PartialEq)]
pub struct Spent {
    pub vendor: String,
    pub calls: Vec<SpentCall>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpentCall {
    /// `None`: the run's calls together, recorded before they were apart.
    pub step: Option<Step>,
    pub model: String,
    pub duration: Duration,
    /// In and out; `None` when no answer came.
    pub tokens: Option<(u64, u64)>,
    /// `None` when it has no price.
    pub cost: Option<f64>,
}

impl Spent {
    /// The calls of the run that wrote `generation`; `None` when it has none,
    /// as for the sample résumé.
    pub fn of_generation(
        connection: &Connection,
        generation: Uuid,
    ) -> Result<Option<Self>, database::Error> {
        let mut vendor = None;
        let calls = connection
            .prepare_cached(
                "SELECT llm_runs.vendor, step, model, llm_calls.duration, input_tokens,
                        output_tokens, cost
                 FROM llm_calls JOIN llm_runs ON llm_runs.id = llm_calls.run_id
                 WHERE llm_runs.generation_id = ?1
                 ORDER BY llm_runs.started_at, llm_calls.rowid",
            )?
            .query_map([generation], |row| {
                vendor = Some(row.get::<_, String>(0)?);
                let tokens: (Option<i64>, Option<i64>) = (row.get(4)?, row.get(5)?);
                Ok(SpentCall {
                    step: row
                        .get::<_, Option<String>>(1)?
                        .as_deref()
                        .and_then(Step::from_stored),
                    model: row.get(2)?,
                    duration: Duration::from_millis(row.get::<_, i64>(3)?.max(0) as u64),
                    tokens: match tokens {
                        (Some(input), Some(output)) => {
                            Some((input.max(0) as u64, output.max(0) as u64))
                        }
                        _ => None,
                    },
                    cost: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(vendor.map(|vendor| Self { vendor, calls }))
    }

    /// What every call cost together; `None` when one has no price, as the
    /// sum would only be a lower bound.
    pub fn total(&self) -> Option<f64> {
        self.calls.iter().map(|call| call.cost).sum()
    }

    /// The résumé was written but no line needed the meaning check, which
    /// checks only lines with words their facts don't have.
    pub fn checked_nothing(&self) -> bool {
        let has = |step| self.calls.iter().any(|call| call.step == Some(step));
        has(Step::Write) && !has(Step::CheckMeaning)
    }

    /// Paid for by the user's ChatGPT plan, so it costs nothing apart.
    pub fn on_plan(&self) -> bool {
        self.vendor == chatgpt::VENDOR
    }
}

#[derive(Debug)]
pub enum Error {
    /// No API key is saved for the provider.
    NoKey,
    Keyring(keyring::Error),
    Unsupported(String),
    Provider(ProviderError),
    /// An OpenAI-compatible server, by name, didn't answer.
    Unreachable(String),
    TimedOut,
    /// The runtime didn't start, or the call's task stopped.
    Runtime(String),
    ChatGpt(chatgpt::Error),
    /// The ChatGPT plan refused the call, with OpenAI's code for why.
    Plan(String),
}

/// OpenAI's code for a ChatGPT plan, or Kron's share of it, used up.
const USAGE_LIMIT: &str = "subscription_sharing_usage_limit_exceeded";

impl Error {
    /// The ChatGPT plan's limit, or Kron's, is reached: the way on is in
    /// ChatGPT's usage settings, not in Kron.
    pub fn is_usage_limit(&self) -> bool {
        matches!(self, Self::Plan(code) if code == USAGE_LIMIT)
    }
}

impl fmt::Display for Error {
    /// What went wrong, for the user. A provider's reply body is left out:
    /// it can quote what was sent.
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NoKey => write!(formatter, "No API key is saved. Add one in Settings."),
            Self::Keyring(error) => {
                write!(
                    formatter,
                    "Couldn't read the API key from the keyring: {error}."
                )
            }
            Self::Unsupported(vendor) => write!(formatter, "{vendor} isn't supported yet."),
            Self::Provider(ProviderError::InvalidAuthentication(_)) => {
                write!(formatter, "The API key was refused. Check it in Settings.")
            }
            Self::Provider(ProviderError::ProviderResponse(response)) => {
                match response.status.map(|status| status.as_u16()) {
                    Some(402) => write!(formatter, "The account's balance has run out."),
                    Some(429) => write!(formatter, "Too many requests. Try again in a minute."),
                    Some(status) if status >= 500 => {
                        write!(
                            formatter,
                            "The provider had a problem ({status}). Try again later."
                        )
                    }
                    Some(status) => {
                        write!(formatter, "The provider refused the request ({status}).")
                    }
                    None => write!(formatter, "The provider refused the request."),
                }
            }
            Self::Provider(ProviderError::Http(_)) => {
                write!(
                    formatter,
                    "Couldn't reach the provider. Check your connection."
                )
            }
            Self::Provider(error) => write!(formatter, "The call failed: {:?}.", error.kind()),
            Self::Unreachable(name) => {
                write!(formatter, "Couldn't reach {name}. Check that it's running.")
            }
            Self::TimedOut => write!(formatter, "The model took too long to answer."),
            Self::Runtime(error) => write!(formatter, "The call couldn't run: {error}."),
            Self::ChatGpt(error) => write!(formatter, "{error}"),
            Self::Plan(code) => match code.as_str() {
                USAGE_LIMIT => write!(
                    formatter,
                    "ChatGPT usage limit reached. Review your plan or Kron's limit in \
                     ChatGPT settings."
                ),
                "subscription_sharing_user_not_eligible" => write!(
                    formatter,
                    "Your ChatGPT account can't share its plan with other apps. Add a \
                     provider with an API key instead."
                ),
                "subscription_sharing_usage_unavailable"
                | "subscription_sharing_user_unavailable" => {
                    write!(
                        formatter,
                        "ChatGPT couldn't check your plan's usage. Try again in a few minutes."
                    )
                }
                "subscription_sharing_invalid_user" => {
                    write!(formatter, "{}", chatgpt::Error::SignInAgain)
                }
                code => write!(formatter, "ChatGPT refused the request ({code})."),
            },
        }
    }
}

/// The provider and model `purpose` runs on: its own, or the default.
pub fn assigned(
    device: &Connection,
    purpose: Purpose,
) -> Result<Option<Assigned>, database::Error> {
    Ok(device
        .prepare_cached(
            "SELECT llm_providers.id, llm_providers.name, llm_providers.vendor,
                    llm_providers.format, llm_providers.base_url, llm_assignments.model
             FROM llm_assignments
             JOIN llm_providers ON llm_providers.id = llm_assignments.provider_id
             WHERE llm_assignments.task IN (?1, 'default')
             ORDER BY llm_assignments.task = 'default'
             LIMIT 1",
        )?
        .query_row([purpose.stored()], |row| {
            Ok(Assigned {
                provider: Provider {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    vendor: row.get(2)?,
                    format: row.get(3)?,
                    base_url: row.get(4)?,
                },
                model: row.get(5)?,
            })
        })
        .optional()?)
}

/// The providers saved on this device, oldest first.
pub fn providers(device: &Connection) -> Result<Vec<Provider>, database::Error> {
    let mut statement = device.prepare_cached(
        "SELECT id, name, vendor, format, base_url FROM llm_providers ORDER BY added_at, id",
    )?;
    let providers = statement
        .query_map([], |row| {
            Ok(Provider {
                id: row.get(0)?,
                name: row.get(1)?,
                vendor: row.get(2)?,
                format: row.get(3)?,
                base_url: row.get(4)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(providers)
}

/// The provider and model `task` is set to, without falling back: `None`
/// for a task means it uses the default, and for the default (`task`
/// `None`) that nothing is set up.
pub fn assignment(
    device: &Connection,
    task: Option<Purpose>,
) -> Result<Option<(Uuid, String)>, database::Error> {
    Ok(device
        .prepare_cached("SELECT provider_id, model FROM llm_assignments WHERE task = ?1")?
        .query_row([task_key(task)], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?)
}

/// Sets the model `task` runs on, the default's with `task` `None`; with
/// `model` `None`, the task goes back to the default.
pub fn assign(
    write: &mut Write,
    task: Option<Purpose>,
    model: Option<(Uuid, &str)>,
) -> Result<(), database::Error> {
    let key = task_key(task);
    let exists: bool = write
        .prepare_device("SELECT EXISTS (SELECT 1 FROM llm_assignments WHERE task = ?1)")?
        .query_row([key], |row| row.get(0))?;
    match (model, exists) {
        (Some((provider, model)), true) => write.update(
            LlmAssignments,
            key,
            &[
                llm_assignments::PROVIDER_ID.value(&provider),
                llm_assignments::MODEL.value(&model),
            ],
        ),
        (Some((provider, model)), false) => write.insert(
            LlmAssignments,
            key,
            &[
                llm_assignments::PROVIDER_ID.value(&provider),
                llm_assignments::MODEL.value(&model),
            ],
        ),
        (None, true) => write.remove(LlmAssignments, key),
        (None, false) => Ok(()),
    }
}

/// Saves a provider: a vendor's, with `base_url` `None`, or an
/// OpenAI-compatible server's. With no default yet, `model` on it becomes
/// the default, so a first provider is ready to use. Returns its id, which
/// names its key.
pub fn add_provider(
    write: &mut Write,
    name: &str,
    vendor: &str,
    base_url: Option<&str>,
    model: &str,
) -> Result<Uuid, database::Error> {
    let provider = Uuid::now_v7();
    let now = write.now();
    let format = base_url.map(|_| CHAT_FORMAT);
    write.insert(
        LlmProviders,
        provider,
        &[
            llm_providers::NAME.value(&name),
            llm_providers::VENDOR.value(&vendor),
            llm_providers::FORMAT.value(&format),
            llm_providers::BASE_URL.value(&base_url),
            llm_providers::ADDED_AT.value(&now),
        ],
    )?;
    if assignment_of(write, None)?.is_none() {
        assign(write, None, Some((provider, model)))?;
    }
    Ok(provider)
}

/// Saves an OpenAI-compatible server at `base_url`, as `add_provider` does.
pub fn add_compatible(
    write: &mut Write,
    name: &str,
    base_url: &str,
    model: &str,
) -> Result<Uuid, database::Error> {
    add_provider(write, name, COMPATIBLE_VENDOR, Some(base_url), model)
}

/// Renames an OpenAI-compatible server, or points it at another address
/// that serves `models`. A task set to a model it doesn't serve moves to the
/// first it does, so it doesn't fail on a model that's gone.
pub fn edit_server(
    write: &mut Write,
    provider: Uuid,
    name: &str,
    base_url: &str,
    models: &[String],
) -> Result<(), database::Error> {
    write.update(
        LlmProviders,
        provider,
        &[
            llm_providers::NAME.value(&name),
            llm_providers::BASE_URL.value(&Some(base_url)),
        ],
    )?;
    let Some(first) = models.first() else {
        return Ok(());
    };
    let gone: Vec<String> = write
        .prepare_device("SELECT task, model FROM llm_assignments WHERE provider_id = ?1")?
        .query_map([provider], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<(String, String)>, _>>()?
        .into_iter()
        .filter(|(_, model)| !models.contains(model))
        .map(|(task, _)| task)
        .collect();
    for task in gone {
        write.set(llm_assignments::MODEL, task.as_str(), &first.as_str())?;
    }
    Ok(())
}

/// Removes the provider; the tasks set to it go back to the default. If
/// the default was on it, another provider's model takes its place, so the
/// others keep working. Its key is `set_key`'s to remove.
pub fn remove_provider(write: &mut Write, provider: Uuid) -> Result<(), database::Error> {
    let tasks: Vec<String> = write
        .prepare_device("SELECT task FROM llm_assignments WHERE provider_id = ?1")?
        .query_map([provider], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    // One at a time rather than by the foreign key's cascade, so views
    // showing them hear that they went.
    for task in tasks {
        write.remove(LlmAssignments, task.as_str())?;
    }
    write.remove(LlmProviders, provider)?;
    if assignment_of(write, None)?.is_none()
        && let Some((provider, model)) = next_default(write)?
    {
        assign(write, None, Some((provider, &model)))?;
    }
    Ok(())
}

/// What becomes the default when it goes: a task's own model, or else the
/// oldest vendor provider's starting model. A server's models aren't known
/// here, so with only servers left the user picks one.
fn next_default(write: &Write) -> Result<Option<(Uuid, String)>, database::Error> {
    let task: Option<(Uuid, String)> = write
        .prepare_device(
            "SELECT provider_id, model FROM llm_assignments WHERE task != 'default'
             ORDER BY task LIMIT 1",
        )?
        .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    if task.is_some() {
        return Ok(task);
    }
    let providers: Vec<(Uuid, String)> = write
        .prepare_device(
            "SELECT id, vendor FROM llm_providers WHERE base_url IS NULL ORDER BY added_at, id",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    Ok(providers
        .into_iter()
        .find_map(|(provider, id)| vendor(&id).map(|vendor| (provider, vendor.model.to_owned()))))
}

/// The models `vendor` offers that rig's catalog lists, by id, with their
/// names: those that read text and can keep to JSON, which leaves out
/// image, embedding and speech models, and none deprecated.
pub fn models(vendor: &str) -> Vec<(String, String)> {
    Catalog::builtin()
        .iter()
        .filter(|spec| {
            spec.provider.vendor() == vendor
                && !spec.deprecated
                && spec.input.text
                && spec.structured_output
        })
        .map(|spec| (spec.id.clone(), spec.display_name.clone()))
        .collect()
}

/// The vendor Kron offers by this id.
pub fn vendor(id: &str) -> Option<&'static Vendor> {
    VENDORS.iter().find(|vendor| vendor.id == id)
}

/// The provider's API key. Blocks on the keyring: call it off the main
/// thread.
pub fn key(provider: Uuid) -> Result<Option<String>, keyring::Error> {
    match keyring::Entry::new(KEYRING_SERVICE, &provider.to_string())?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Saves the provider's API key, or with `None` removes it. Blocks on the
/// keyring: call it off the main thread.
pub fn set_key(provider: Uuid, key: Option<&str>) -> Result<(), keyring::Error> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, &provider.to_string())?;
    match key {
        Some(key) => entry.set_password(key),
        None => match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error),
        },
    }
}

/// Sends `request` to the model a task is assigned.
pub fn complete(
    assigned: &Assigned,
    request: CompletionRequest,
    cx: &App,
) -> Task<Result<Reply, Error>> {
    let Some(runtime) = cx.try_global::<Runtime>() else {
        return Task::ready(Err(Error::Runtime("no runtime".into())));
    };
    let handle = runtime.0.handle().clone();
    let assigned = assigned.clone();
    cx.background_spawn(async move {
        let key = match key(assigned.provider.id).map_err(Error::Keyring)? {
            Some(key) => key,
            None if assigned.provider.is_compatible() => String::new(),
            None => return Err(Error::NoKey),
        };
        handle
            .spawn(async move { call(&assigned, key, request).await })
            .await
            .map_err(|error| Error::Runtime(error.to_string()))?
    })
}

/// Starts signing in with ChatGPT: the link for the browser to open, and the
/// sign-in, finished once the browser comes back, with the models the
/// account can use.
pub fn sign_in_with_chatgpt(
    registration: &chatgpt::Registration,
    cx: &App,
) -> Result<(String, Task<Result<ChatGptSignIn, Error>>), String> {
    let Some(runtime) = cx.try_global::<Runtime>() else {
        return Err(Error::Runtime("no runtime".into()).to_string());
    };
    let handle = runtime.0.handle().clone();
    let attempt = chatgpt::start(registration).map_err(|error| error.to_string())?;
    let url = attempt.url.clone();
    let finished = cx.background_spawn(async move {
        handle
            .spawn(async move {
                let signed_in = chatgpt::finish(attempt).await.map_err(Error::ChatGpt)?;
                let models = match signed_in.plan {
                    true => chatgpt::models(signed_in.access_token())
                        .await
                        .map_err(Error::ChatGpt)?,
                    false => Vec::new(),
                };
                Ok(ChatGptSignIn { signed_in, models })
            })
            .await
            .map_err(|error| Error::Runtime(error.to_string()))?
    });
    Ok((url, finished))
}

/// A finished sign-in with ChatGPT.
pub struct ChatGptSignIn {
    pub signed_in: chatgpt::SignedIn,
    /// (slug, display name), in OpenAI's order; empty without the plan.
    pub models: Vec<(String, String)>,
}

/// Keeps a sign-in's access token for the provider it was added as, so its
/// first call doesn't spend a refresh.
pub fn remember_chatgpt(provider: Uuid, signed_in: chatgpt::SignedIn, cx: &App) -> Task<()> {
    let Some(runtime) = cx.try_global::<Runtime>() else {
        return Task::ready(());
    };
    let handle = runtime.0.handle().clone();
    cx.background_spawn(async move {
        if let Err(error) = handle
            .spawn(async move { chatgpt::remember(provider, &signed_in).await })
            .await
        {
            eprintln!("chatgpt: couldn't keep the access token: {error}");
        }
    })
}

/// The models a signed-in ChatGPT provider's account can use.
pub fn chatgpt_models(provider: Uuid, cx: &App) -> Task<Result<Vec<(String, String)>, Error>> {
    let Some(runtime) = cx.try_global::<Runtime>() else {
        return Task::ready(Err(Error::Runtime("no runtime".into())));
    };
    let handle = runtime.0.handle().clone();
    cx.background_spawn(async move {
        let stored = key(provider).map_err(Error::Keyring)?.ok_or(Error::NoKey)?;
        let credentials = chatgpt::Credentials::from_stored(&stored).map_err(Error::ChatGpt)?;
        handle
            .spawn(async move {
                let token = chatgpt::access_token(provider, credentials, |rotated| {
                    set_key(provider, Some(&rotated.stored()))
                })
                .await
                .map_err(Error::ChatGpt)?;
                chatgpt::models(&token).await.map_err(Error::ChatGpt)
            })
            .await
            .map_err(|error| Error::Runtime(error.to_string()))?
    })
}

/// Signs a ChatGPT provider out: ends its session at OpenAI, then removes
/// its tokens from the keyring whether or not OpenAI confirmed. `Ok(false)`:
/// it didn't, so the user can disconnect Kron in ChatGPT's settings.
pub fn sign_out_of_chatgpt(provider: Uuid, cx: &App) -> Task<Result<bool, Error>> {
    let Some(runtime) = cx.try_global::<Runtime>() else {
        return Task::ready(Err(Error::Runtime("no runtime".into())));
    };
    let handle = runtime.0.handle().clone();
    cx.background_spawn(async move {
        let stored = key(provider).map_err(Error::Keyring)?;
        let credentials = stored.and_then(|stored| chatgpt::Credentials::from_stored(&stored).ok());
        let revoked = match credentials {
            Some(credentials) => {
                let revoked = handle
                    .spawn(async move { chatgpt::sign_out(provider, &credentials).await })
                    .await;
                match revoked {
                    Ok(Ok(())) => true,
                    Ok(Err(error)) => {
                        eprintln!("chatgpt: couldn't end the session at OpenAI: {error}");
                        false
                    }
                    Err(error) => {
                        eprintln!("chatgpt: couldn't end the session at OpenAI: {error}");
                        false
                    }
                }
            }
            // Nothing to end.
            None => true,
        };
        set_key(provider, None).map_err(Error::Keyring)?;
        Ok(revoked)
    })
}

/// Asks the OpenAI-compatible server at `base_url` which models it serves,
/// by id. Only such servers are asked: a vendor's models are rig's catalog's.
pub fn served_models(
    base_url: String,
    key: Option<String>,
    cx: &App,
) -> Task<Result<Vec<String>, Error>> {
    let Some(runtime) = cx.try_global::<Runtime>() else {
        return Task::ready(Err(Error::Runtime("no runtime".into())));
    };
    let handle = runtime.0.handle().clone();
    cx.background_spawn(async move {
        handle
            .spawn(async move {
                let listed = tokio::time::timeout(
                    LISTING_TIMEOUT,
                    compatible(&base_url, key.unwrap_or_default())
                        .client()
                        .list_models(),
                )
                .await
                .map_err(|_| Error::TimedOut)?
                .map_err(Error::Provider)?;
                let mut models: Vec<String> =
                    listed.data.into_iter().map(|model| model.id).collect();
                models.sort();
                Ok(models)
            })
            .await
            .map_err(|error| Error::Runtime(error.to_string()))?
    })
}

/// Sends `request` with `key`. Runs on a tokio runtime, as rig's HTTP client
/// needs.
pub async fn call(
    assigned: &Assigned,
    key: String,
    mut request: CompletionRequest,
) -> Result<Reply, Error> {
    let provider = &assigned.provider;
    if provider.is_chatgpt() {
        return call_chatgpt(assigned, key, request).await;
    }
    if provider.vendor == "deepseek" && provider.base_url.is_none() {
        // DeepSeek's JSON mode. Other vendors name theirs differently, and
        // compatible servers don't all have one: they're given the shape the
        // preamble spells out, and `Answer::parse` reads past a code block.
        request.additional_params =
            Some(serde_json::json!({ "response_format": { "type": "json_object" } }));
    }
    let model = completion_model(provider, key, &assigned.model)
        .ok_or_else(|| Error::Unsupported(provider.vendor.clone()))?;
    let response = tokio::time::timeout(TIMEOUT, retried(|| model.call(request.clone())))
        .await
        .map_err(|_| Error::TimedOut)?
        .map_err(|error| match error {
            // A server on the user's machine that isn't running, more
            // likely than their connection being down.
            ProviderError::Http(_) if provider.is_compatible() => {
                Error::Unreachable(provider.name.clone())
            }
            error => Error::Provider(error),
        })?;
    let catalog_price = || {
        // A compatible server's models aren't the catalog's, whatever
        // they're named.
        if provider.is_compatible() {
            return None;
        }
        Catalog::builtin()
            .resolve(&format!("{}/{}", provider.vendor, assigned.model))
            .and_then(|spec| spec.pricing)
            .and_then(|pricing| pricing.cost(&response.usage))
            .map(|cost| cost.total)
    };
    let cost = response
        .usage
        .cost
        .as_ref()
        .map(|cost| cost.total)
        .or_else(catalog_price);
    Ok(Reply { response, cost })
}

/// The provider's `model`, called with `key`; `None` for a vendor rig
/// doesn't know.
/// `call`, on the user's ChatGPT plan. The plan takes no temperature or
/// output cap and keeps nothing, and its replies are streamed. It's paid for
/// by the plan, so there's no cost to record.
async fn call_chatgpt(
    assigned: &Assigned,
    stored: String,
    request: CompletionRequest,
) -> Result<Reply, Error> {
    let provider = assigned.provider.id;
    let credentials = chatgpt::Credentials::from_stored(&stored).map_err(Error::ChatGpt)?;
    let token = chatgpt::access_token(provider, credentials, |rotated| {
        set_key(provider, Some(&rotated.stored()))
    })
    .await
    .map_err(Error::ChatGpt)?;
    let request = for_plan(request);
    let model = plan_config(token)
        .client()
        .completion(&assigned.model)
        .erase();
    let streamed = retried(|| async { model.stream(request.clone())?.finish().await });
    let response = tokio::time::timeout(TIMEOUT, streamed)
        .await
        .map_err(|_| Error::TimedOut)?
        .map_err(|error| {
            let code = match &error {
                ProviderError::ProviderResponse(response)
                | ProviderError::InvalidAuthentication(response) => {
                    chatgpt::error_code(response.body.as_bytes())
                }
                _ => None,
            };
            match code {
                Some(code) => Error::Plan(code),
                None => Error::Provider(error),
            }
        })?;
    Ok(Reply {
        response,
        cost: None,
    })
}

/// Runs `attempt`, again after a pause when it never reached the provider. A
/// call that reached it isn't resent: it may have been answered, and paid for.
async fn retried<T, F>(attempt: impl Fn() -> F) -> Result<T, ProviderError>
where
    F: Future<Output = Result<T, ProviderError>>,
{
    let mut delays = RETRY_DELAYS.iter();
    loop {
        match attempt().await {
            Err(error) if never_reached(&error) => match delays.next() {
                Some(delay) => {
                    eprintln!("llm: a call never reached the provider; trying again");
                    tokio::time::sleep(*delay).await;
                }
                None => return Err(error),
            },
            result => return result,
        }
    }
}

/// The request failed to look up or connect to the provider, so it never
/// arrived.
fn never_reached(error: &ProviderError) -> bool {
    let ProviderError::Http(error) = error else {
        return false;
    };
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&**error);
    while let Some(cause) = source {
        if cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_connect)
        {
            return true;
        }
        source = cause.source();
    }
    false
}

/// The plan's endpoint, on rig's wire for ChatGPT plans: always streamed,
/// nothing stored, and a reply that names no content type accepted, as the
/// plan's don't. Only the wire: the endpoint, token and client are Kron's.
fn plan_config(token: String) -> OpenAIConfig {
    let mut config = OpenAIConfig::new(token)
        .with_base_url(chatgpt::API)
        .with_route(Route::Responses);
    config.dialect.quirks.responses.contract = ResponsesContract::Codex;
    config
}

/// The request as the plan takes it: no temperature or output cap, which it
/// rejects, and nothing kept on OpenAI's side. Its models can't turn
/// reasoning off, so the least they offer stands in.
fn for_plan(mut request: CompletionRequest) -> CompletionRequest {
    if request.options.reasoning == Some(Reasoning::Off) {
        request.options.reasoning = Some(Reasoning::Effort(Effort::Low));
    }
    request.temperature = None;
    request.max_tokens = None;
    request.additional_params = Some(serde_json::json!({ "store": false }));
    request
}

fn completion_model(provider: &Provider, key: String, model: &str) -> Option<DynModel<Completion>> {
    if let Some(base_url) = &provider.base_url {
        return Some(compatible(base_url, key).client().completion(model).erase());
    }
    let config = ProviderId::resolve(&provider.vendor).ok()?.config(key)?;
    Some(match config {
        ProviderConfig::OpenAi(config) => config.client().completion(model).erase(),
        ProviderConfig::Anthropic(config) => config.client().completion(model).erase(),
        ProviderConfig::Gemini(config) => config.client().completion(model).erase(),
    })
}

/// An OpenAI-compatible server at `base_url`. An empty key sends no
/// `Authorization` header, which a local server may refuse to see.
fn compatible(base_url: &str, key: String) -> OpenAIConfig {
    OpenAIConfig::new(key)
        .with_auth(Auth::OptionalBearer)
        .with_base_url(base_url)
        .with_route(Route::Chat)
}

fn task_key(task: Option<Purpose>) -> &'static str {
    task.map_or("default", Purpose::stored)
}

/// `assignment`, read inside a command.
fn assignment_of(
    write: &Write,
    task: Option<Purpose>,
) -> Result<Option<(Uuid, String)>, database::Error> {
    Ok(write
        .prepare_device("SELECT provider_id, model FROM llm_assignments WHERE task = ?1")?
        .query_row([task_key(task)], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?)
}

/// What a run cost, as the user reads it: "under 1¢", "3¢", "$1.20".
pub fn cost_label(cost: f64) -> String {
    let cents = (cost * 100.).round();
    if cents < 1. {
        "under 1¢".to_owned()
    } else if cents < 100. {
        format!("{cents}¢")
    } else {
        format!("${:.2}", cents / 100.)
    }
}

/// What a call cost, closer than `cost_label`: "0.21¢", "4.5¢", "$1.20".
pub fn cost_detail(cost: f64) -> String {
    let cents = cost * 100.;
    if cents < 1. {
        format!("{cents:.2}¢")
    } else if cents < 100. {
        format!("{cents:.1}¢")
    } else {
        format!("${:.2}", cents / 100.)
    }
}

/// A count of tokens, as the user reads it: "850", "4.2k", "1.3M".
pub fn tokens_label(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{:.1}k", tokens as f64 / 1e3),
        _ => format!("{:.1}M", tokens as f64 / 1e6),
    }
}

/// Records a run and its calls: what they took and cost, never what was
/// sent or answered.
pub fn record(write: &mut Write, run: &Run) -> Result<(), database::Error> {
    let id = Uuid::now_v7();
    write.insert(
        LlmRuns,
        id,
        &[
            llm_runs::TASK.value(&run.purpose.stored()),
            llm_runs::STARTED_AT.value(&run.started_at),
            llm_runs::DURATION.value(&(run.duration.as_millis() as i64)),
            llm_runs::OUTCOME.value(&if run.done { "done" } else { "failed" }),
            llm_runs::PROVIDER.value(&run.assigned.provider.name),
            llm_runs::VENDOR.value(&run.assigned.provider.vendor),
            llm_runs::DOCUMENT_ID.value(&run.document),
            llm_runs::JOB_ID.value(&run.job),
            llm_runs::GENERATION_ID.value(&run.generation),
        ],
    )?;
    for call in run.calls {
        let usage = call.reply.map(|reply| reply.response.usage);
        let tokens = |count: fn(&rig_core::completion::Usage) -> Option<u64>| {
            usage.as_ref().map(|usage| count(usage).unwrap_or(0) as i64)
        };
        let model = call
            .reply
            .and_then(|reply| reply.response.model())
            .unwrap_or(&run.assigned.model)
            .to_owned();
        write.insert(
            LlmCalls,
            Uuid::now_v7(),
            &[
                llm_calls::RUN_ID.value(&id),
                llm_calls::STEP.value(&Some(call.step.stored())),
                llm_calls::MODEL.value(&model),
                llm_calls::DURATION.value(&(call.duration.as_millis() as i64)),
                llm_calls::INPUT_TOKENS.value(&tokens(|usage| usage.input_tokens)),
                llm_calls::CACHED_INPUT_TOKENS.value(&tokens(|usage| usage.cached_input_tokens)),
                llm_calls::CACHE_WRITTEN_TOKENS
                    .value(&tokens(|usage| usage.cache_creation_input_tokens)),
                llm_calls::OUTPUT_TOKENS.value(&tokens(|usage| usage.output_tokens)),
                llm_calls::REASONING_TOKENS.value(&tokens(|usage| usage.reasoning_tokens)),
                llm_calls::COST.value(&call.reply.and_then(|reply| reply.cost)),
            ],
        )?;
    }
    Ok(())
}

/// For the live tests: what `purpose` runs on in the development data, with
/// its key from the keyring. `KRON_LIVE_VENDOR` picks another of its
/// providers by vendor, and `KRON_LIVE_MODEL` another model, to compare them.
#[cfg(test)]
pub fn live_assigned(purpose: Purpose) -> Result<(Assigned, String), Box<dyn std::error::Error>> {
    let device = dirs::data_dir()
        .ok_or("no data directory")?
        .join("kron-dev/device.sqlite");
    let device = Connection::open_with_flags(device, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut assigned = assigned(&device, purpose)
        .map_err(|error| format!("{error:?}"))?
        .ok_or("no provider set up in the development data")?;
    if let Ok(vendor) = std::env::var("KRON_LIVE_VENDOR") {
        assigned.provider = providers(&device)
            .map_err(|error| format!("{error:?}"))?
            .into_iter()
            .find(|provider| provider.vendor == vendor)
            .ok_or_else(|| format!("no {vendor} provider in the development data"))?;
    }
    if let Ok(model) = std::env::var("KRON_LIVE_MODEL") {
        assigned.model = model;
    }
    let key = key(assigned.provider.id)?.ok_or("no key saved for the provider")?;
    println!("== on {} {}", assigned.provider.vendor, assigned.model);
    Ok((assigned, key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Writer;

    #[test]
    fn a_call_that_never_reached_the_provider_is_sent_again()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::io::{BufRead, BufReader, Read, Write};
        let port = std::net::TcpListener::bind("127.0.0.1:0")?
            .local_addr()?
            .port();
        let server = std::thread::spawn(move || -> std::io::Result<()> {
            std::thread::sleep(Duration::from_millis(300));
            let listener = std::net::TcpListener::bind(("127.0.0.1", port))?;
            let (stream, _) = listener.accept()?;
            let mut reader = BufReader::new(stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line)?;
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            reader.read_exact(&mut vec![0; length])?;
            let body = r#"{"id":"1","object":"chat.completion","created":0,"model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"{}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
            write!(
                reader.get_mut(),
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        });
        let assigned = Assigned {
            provider: Provider {
                id: Uuid::new_v4(),
                name: "Remote".to_owned(),
                vendor: "openai".to_owned(),
                format: None,
                base_url: Some(format!("http://127.0.0.1:{port}/v1")),
            },
            model: "m".to_owned(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime
            .block_on(call(
                &assigned,
                String::new(),
                crate::model::extraction::request("A résumé."),
            ))
            .map_err(|error| format!("{error:?}"))?;
        server.join().map_err(|_| "the server panicked")??;
        Ok(())
    }

    #[test]
    fn the_plan_is_sent_no_temperature_or_cap_and_keeps_nothing() {
        let request = for_plan(crate::model::extraction::request("A résumé."));
        assert_eq!(request.temperature, None);
        assert_eq!(request.max_tokens, None);
        assert_eq!(
            request.options.reasoning,
            Some(Reasoning::Effort(Effort::Low)),
            "gpt-6.2-sol refuses effort none"
        );
        assert_eq!(
            request.additional_params,
            Some(serde_json::json!({ "store": false }))
        );
    }

    #[test]
    fn a_used_up_plan_is_told_apart_and_points_to_chatgpts_settings() {
        let limit = Error::Plan(USAGE_LIMIT.to_owned());
        assert!(limit.is_usage_limit());
        assert!(limit.to_string().contains("ChatGPT settings"));
        assert!(!Error::Plan("subscription_sharing_user_not_eligible".to_owned()).is_usage_limit());
        assert!(!Error::TimedOut.is_usage_limit());
    }

    #[test]
    fn the_plan_is_called_at_openais_api_on_the_wire_that_reads_its_replies() {
        let config = plan_config("token".to_owned());
        assert_eq!(config.base_url, chatgpt::API);
        assert_eq!(config.route, Some(Route::Responses));
        assert_eq!(
            config.dialect.quirks.responses.contract,
            ResponsesContract::Codex,
            "the plan's streamed replies name no content type"
        );
    }

    fn deepseek(write: &mut Write) -> Result<Uuid, database::Error> {
        add_provider(write, "DeepSeek", "deepseek", None, "deepseek-flash")
    }

    #[test]
    fn the_first_provider_becomes_the_default_and_later_ones_dont() -> Result<(), database::Error> {
        let mut writer = Writer::open_in_memory()?;
        assert_eq!(assigned(writer.device(), Purpose::ExtractFacts)?, None);
        let (first, _) = writer.execute(deepseek)?;
        let (second, _) = writer.execute(|write| {
            add_compatible(write, "Ollama", "http://localhost:11434/v1", "qwen3")
        })?;
        assert_eq!(
            assignment(writer.device(), None)?,
            Some((first, "deepseek-flash".to_owned()))
        );
        let providers = providers(writer.device())?;
        assert_eq!(
            providers
                .iter()
                .map(|provider| provider.id)
                .collect::<Vec<_>>(),
            vec![first, second]
        );
        assert!(providers[1].is_compatible());
        assert_eq!(providers[1].format.as_deref(), Some(CHAT_FORMAT));
        Ok(())
    }

    #[test]
    fn a_task_runs_on_its_own_model_until_it_goes_back_to_the_default()
    -> Result<(), database::Error> {
        let mut writer = Writer::open_in_memory()?;
        let (deepseek, _) = writer.execute(deepseek)?;
        let (server, _) = writer.execute(|write| {
            add_compatible(write, "Ollama", "http://localhost:11434/v1", "qwen3")
        })?;
        writer.execute(|write| {
            assign(
                write,
                Some(Purpose::GenerateResume),
                Some((server, "qwen3")),
            )
        })?;
        let runs_on = |writer: &Writer, purpose| {
            assigned(writer.device(), purpose)
                .map(|assigned| assigned.map(|assigned| (assigned.provider.id, assigned.model)))
        };
        assert_eq!(
            runs_on(&writer, Purpose::GenerateResume)?,
            Some((server, "qwen3".to_owned()))
        );
        assert_eq!(
            runs_on(&writer, Purpose::ExtractFacts)?,
            Some((deepseek, "deepseek-flash".to_owned())),
            "the default"
        );
        writer.execute(|write| assign(write, Some(Purpose::GenerateResume), None))?;
        assert_eq!(
            runs_on(&writer, Purpose::GenerateResume)?,
            Some((deepseek, "deepseek-flash".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn removing_a_provider_sends_its_tasks_back_to_the_default() -> Result<(), database::Error> {
        let mut writer = Writer::open_in_memory()?;
        let (deepseek, _) = writer.execute(deepseek)?;
        let (server, _) = writer.execute(|write| {
            add_compatible(write, "Ollama", "http://localhost:11434/v1", "qwen3")
        })?;
        writer
            .execute(|write| assign(write, Some(Purpose::ExtractFacts), Some((server, "qwen3"))))?;
        let (_, changed) = writer.execute(|write| remove_provider(write, server))?;
        assert!(
            changed
                .iter()
                .any(|change| change.table == "llm_assignments"),
            "the pickers hear of it"
        );
        assert_eq!(
            assignment(writer.device(), Some(Purpose::ExtractFacts))?,
            None
        );
        assert_eq!(
            assigned(writer.device(), Purpose::ExtractFacts)?.map(|assigned| assigned.provider.id),
            Some(deepseek)
        );
        writer.execute(|write| remove_provider(write, deepseek))?;
        assert_eq!(assigned(writer.device(), Purpose::ExtractFacts)?, None);
        assert!(providers(writer.device())?.is_empty());
        Ok(())
    }

    #[test]
    fn an_edited_server_keeps_the_models_it_still_serves() -> Result<(), database::Error> {
        let mut writer = Writer::open_in_memory()?;
        let (server, _) = writer.execute(|write| {
            add_compatible(write, "Ollama", "http://localhost:11434/v1", "qwen3")
        })?;
        writer.execute(|write| {
            assign(
                write,
                Some(Purpose::GenerateResume),
                Some((server, "llama3.3")),
            )
        })?;
        writer.execute(|write| {
            edit_server(
                write,
                server,
                "Home server",
                "http://192.168.1.5:11434/v1",
                &["mistral-small".to_owned(), "qwen3".to_owned()],
            )
        })?;
        let Some(assigned) = assigned(writer.device(), Purpose::ExtractFacts)? else {
            panic!("the server is still the default");
        };
        assert_eq!(assigned.provider.id, server);
        assert_eq!(assigned.provider.name, "Home server");
        assert_eq!(
            assigned.provider.base_url.as_deref(),
            Some("http://192.168.1.5:11434/v1")
        );
        assert_eq!(assigned.model, "qwen3", "still served, so kept");
        assert_eq!(
            assignment(writer.device(), Some(Purpose::GenerateResume))?,
            Some((server, "mistral-small".to_owned())),
            "gone, so moved to the first served"
        );
        Ok(())
    }

    #[test]
    fn removing_the_defaults_provider_hands_the_default_to_another() -> Result<(), database::Error>
    {
        let mut writer = Writer::open_in_memory()?;
        let (deepseek, _) = writer.execute(deepseek)?;
        let (anthropic, _) = writer.execute(|write| {
            add_provider(write, "Anthropic", "anthropic", None, "claude-haiku-4-5")
        })?;
        writer.execute(|write| remove_provider(write, deepseek))?;
        assert_eq!(
            assignment(writer.device(), None)?,
            Some((anthropic, "claude-haiku-4-5".to_owned()))
        );
        // A task's own model is kept in its place, over a starting model.
        let (server, _) = writer.execute(|write| {
            add_compatible(write, "Ollama", "http://localhost:11434/v1", "qwen3")
        })?;
        writer.execute(|write| {
            assign(
                write,
                Some(Purpose::GenerateResume),
                Some((server, "qwen3")),
            )
        })?;
        writer.execute(|write| remove_provider(write, anthropic))?;
        assert_eq!(
            assignment(writer.device(), None)?,
            Some((server, "qwen3".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn a_server_that_isnt_running_is_named_in_the_error() {
        let assigned = Assigned {
            provider: Provider {
                id: Uuid::nil(),
                name: "Ollama".to_owned(),
                vendor: COMPATIBLE_VENDOR.to_owned(),
                format: Some(CHAT_FORMAT.to_owned()),
                // Port 1: nothing listens there.
                base_url: Some("http://127.0.0.1:1/v1".to_owned()),
            },
            model: "qwen3".to_owned(),
        };
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => panic!("couldn't start a runtime: {error}"),
        };
        let reply = runtime.block_on(call(
            &assigned,
            String::new(),
            CompletionRequest::new("Hello".to_owned()),
        ));
        assert_eq!(
            reply.err().map(|error| error.to_string()),
            Some("Couldn't reach Ollama. Check that it's running.".to_owned())
        );
    }

    /// The catalog comes with rig: an update that drops a model a vendor
    /// starts on would leave a new provider's default unlisted.
    #[test]
    fn every_vendor_starts_on_a_model_its_catalog_lists() {
        for vendor in VENDORS {
            let models = models(vendor.id);
            assert!(
                models.iter().any(|(id, _)| id == vendor.model),
                "{} isn't among {}'s models",
                vendor.model,
                vendor.name
            );
            assert!(ProviderId::resolve(vendor.id).is_ok(), "{}", vendor.id);
        }
    }

    #[test]
    fn a_failed_call_is_recorded_without_tokens_or_cost() -> Result<(), database::Error> {
        let mut writer = Writer::open_in_memory()?;
        writer.execute(deepseek)?;
        let Some(assigned) = assigned(writer.device(), Purpose::ExtractFacts)? else {
            panic!("the provider just added");
        };
        writer.execute(|write| {
            let document = crate::model::commands::import_document(write, None, "Built things.")?;
            record(
                write,
                &Run {
                    purpose: Purpose::ExtractFacts,
                    assigned: &assigned,
                    started_at: 1,
                    duration: Duration::from_millis(1500),
                    calls: &[Call {
                        step: Step::ExtractFacts,
                        duration: Duration::from_millis(1500),
                        reply: None,
                    }],
                    done: false,
                    document: Some(document.0),
                    job: None,
                    generation: None,
                },
            )
        })?;
        type Recorded = (String, String, String, i64, Option<i64>, Option<f64>);
        let recorded: Recorded = writer.kron().query_row(
            "SELECT outcome, step, model, llm_calls.duration, output_tokens, cost
             FROM llm_runs JOIN llm_calls ON llm_calls.run_id = llm_runs.id",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )?;
        assert_eq!(
            recorded,
            (
                "failed".to_owned(),
                "extract_facts".to_owned(),
                "deepseek-flash".to_owned(),
                1500,
                None,
                None
            )
        );
        Ok(())
    }

    #[test]
    fn a_calls_cost_and_tokens_read_closer() {
        assert_eq!(cost_detail(0.0021), "0.21¢");
        assert_eq!(cost_detail(0.045), "4.5¢");
        assert_eq!(cost_detail(1.2), "$1.20");
        assert_eq!(tokens_label(850), "850");
        assert_eq!(tokens_label(4_240), "4.2k");
        assert_eq!(tokens_label(1_300_000), "1.3M");
    }

    #[test]
    fn costs_read_in_cents_under_a_dollar() {
        assert_eq!(cost_label(0.0004), "under 1¢");
        assert_eq!(cost_label(0.012), "1¢");
        assert_eq!(cost_label(0.237), "24¢");
        assert_eq!(cost_label(0.996), "$1.00");
        assert_eq!(cost_label(1.2), "$1.20");
    }
}
