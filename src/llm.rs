// Calls to AI models. The provider and the model each task runs on are per
// device (`llm_providers`, `llm_assignments` in device.sqlite); a provider's
// API key is in the OS keyring, named by its id, never in the database.
// Calls go through rig, on a tokio runtime of their own, since rig's HTTP
// client needs one and GPUI doesn't run one. Nothing here logs what's sent or
// answered.

use std::fmt;
use std::time::Duration;

use gpui_kit::{App, AppContext, Global, Task};
use rig_core::ProviderError;
use rig_core::catalog::Catalog;
use rig_core::completion::{CompletionRequest, CompletionResponse};
use rig_core::driver::DynModel;
use rig_core::operation::Completion;
use rig_core::providers::openai::wire::Auth;
use rig_core::providers::openai::{OpenAIConfig, Route};
use rig_core::providers::registry::{ProviderConfig, ProviderId};
use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use crate::database::schema::{
    LlmAssignments, LlmProviders, LlmRuns, llm_assignments, llm_providers, llm_runs,
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

/// A call made, to record in `llm_runs`.
pub struct Run<'a> {
    pub purpose: Purpose,
    pub assigned: &'a Assigned,
    /// In ms since the Unix epoch.
    pub started_at: i64,
    pub duration: Duration,
    /// The answer, if one came.
    pub reply: Option<&'a Reply>,
    /// Whether the answer was used: one that couldn't be read wasn't.
    pub done: bool,
    /// What it ran for: a document's facts, or a job's résumé, and the
    /// résumé made.
    pub document: Option<Uuid>,
    pub job: Option<Uuid>,
    pub generation: Option<Uuid>,
}

#[derive(Debug)]
pub enum Error {
    /// No API key is saved for the provider.
    NoKey,
    Keyring(keyring::Error),
    Unsupported(String),
    Provider(ProviderError),
    TimedOut,
    /// The runtime didn't start, or the call's task stopped.
    Runtime(String),
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
            Self::TimedOut => write!(formatter, "The model took too long to answer."),
            Self::Runtime(error) => write!(formatter, "The call couldn't run: {error}."),
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

/// Removes the provider; the tasks set to it go back to the default. Its
/// key is `set_key`'s to remove.
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
    write.remove(LlmProviders, provider)
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
    if provider.vendor == "deepseek" && provider.base_url.is_none() {
        // DeepSeek's JSON mode. Other vendors name theirs differently, and
        // compatible servers don't all have one: they're given the shape the
        // preamble spells out, and `Answer::parse` reads past a code block.
        request.additional_params =
            Some(serde_json::json!({ "response_format": { "type": "json_object" } }));
    }
    let model = completion_model(provider, key, &assigned.model)
        .ok_or_else(|| Error::Unsupported(provider.vendor.clone()))?;
    let response = tokio::time::timeout(TIMEOUT, model.call(request))
        .await
        .map_err(|_| Error::TimedOut)?
        .map_err(Error::Provider)?;
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

/// Records a call: what it took and cost, never what was sent or answered.
/// A call with no price counts as unpriced, at no cost: the total is a
/// lower bound.
pub fn record(write: &mut Write, run: &Run) -> Result<(), database::Error> {
    let usage = run.reply.map(|reply| &reply.response.usage);
    let tokens = |count: fn(&rig_core::completion::Usage) -> Option<u64>| {
        usage.and_then(count).unwrap_or(0) as i64
    };
    let model = run
        .reply
        .and_then(|reply| reply.response.model())
        .unwrap_or(&run.assigned.model)
        .to_owned();
    let cost = run.reply.and_then(|reply| reply.cost);
    let duration = run.duration.as_millis() as i64;
    write.insert(
        LlmRuns,
        Uuid::now_v7(),
        &[
            llm_runs::TASK.value(&run.purpose.stored()),
            llm_runs::STARTED_AT.value(&run.started_at),
            llm_runs::DURATION.value(&duration),
            llm_runs::OUTCOME.value(&if run.done { "done" } else { "failed" }),
            llm_runs::PROVIDER.value(&run.assigned.provider.name),
            llm_runs::VENDOR.value(&run.assigned.provider.vendor),
            llm_runs::MODEL.value(&model),
            llm_runs::CALLS.value(&1_i64),
            llm_runs::INPUT_TOKENS.value(&Some(tokens(|usage| usage.input_tokens))),
            llm_runs::CACHED_INPUT_TOKENS.value(&Some(tokens(|usage| usage.cached_input_tokens))),
            llm_runs::CACHE_WRITTEN_TOKENS
                .value(&Some(tokens(|usage| usage.cache_creation_input_tokens))),
            llm_runs::OUTPUT_TOKENS.value(&Some(tokens(|usage| usage.output_tokens))),
            llm_runs::REASONING_TOKENS.value(&Some(tokens(|usage| usage.reasoning_tokens))),
            llm_runs::COST.value(&Some(cost.unwrap_or(0.))),
            llm_runs::UNPRICED_CALLS.value(&Some(i64::from(cost.is_none()))),
            llm_runs::DOCUMENT_ID.value(&run.document),
            llm_runs::JOB_ID.value(&run.job),
            llm_runs::GENERATION_ID.value(&run.generation),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Writer;

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
                    reply: None,
                    done: false,
                    document: Some(document.0),
                    job: None,
                    generation: None,
                },
            )
        })?;
        let run: (String, String, i64, i64, f64, i64) = writer.kron().query_row(
            "SELECT outcome, model, duration, output_tokens, cost, unpriced_calls FROM llm_runs",
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
            run,
            (
                "failed".to_owned(),
                "deepseek-flash".to_owned(),
                1500,
                0,
                0.,
                1
            )
        );
        Ok(())
    }
}
