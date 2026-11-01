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
use rig_core::providers::openai::OpenAIConfig;
use rig_core::providers::openai::wire::DEEPSEEK;
use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use crate::database::schema::{
    LlmAssignments, LlmProviders, LlmRuns, llm_assignments, llm_providers, llm_runs,
};
use crate::database::{self, Write};

/// The keyring service keys are kept under, each named by its provider's id.
const KEYRING_SERVICE: &str = "Kron";
/// The vendors Kron can call, by rig's vendor id, with their names.
pub const VENDORS: &[(&str, &str)] = &[("deepseek", "DeepSeek")];
/// A new provider's model, until the user picks another: DeepSeek points it
/// at its latest Flash model.
pub const DEFAULT_MODEL: &str = "deepseek-flash";
/// Ours: a CV's facts come back in well under this; past it, the call has
/// hung.
const TIMEOUT: Duration = Duration::from_secs(180);

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

/// A provider, with the model a task runs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provider {
    pub id: Uuid,
    pub name: String,
    pub vendor: String,
    pub base_url: Option<String>,
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
    pub provider: &'a Provider,
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
) -> Result<Option<Provider>, database::Error> {
    Ok(device
        .prepare_cached(
            "SELECT llm_providers.id, llm_providers.name, llm_providers.vendor,
                    llm_providers.base_url, llm_assignments.model
             FROM llm_assignments
             JOIN llm_providers ON llm_providers.id = llm_assignments.provider_id
             WHERE llm_assignments.task IN (?1, 'default')
             ORDER BY llm_assignments.task = 'default'
             LIMIT 1",
        )?
        .query_row([purpose.stored()], |row| {
            Ok(Provider {
                id: row.get(0)?,
                name: row.get(1)?,
                vendor: row.get(2)?,
                base_url: row.get(3)?,
                model: row.get(4)?,
            })
        })
        .optional()?)
}

/// The models `vendor` offers that rig's catalog lists and hasn't marked
/// deprecated, by id, with their names.
pub fn models(vendor: &str) -> Vec<(String, String)> {
    Catalog::builtin()
        .iter()
        .filter(|spec| spec.provider.vendor() == vendor && !spec.deprecated)
        .map(|spec| (spec.id.clone(), spec.display_name.clone()))
        .collect()
}

/// Makes `vendor` the default provider, with `model`: the provider saved
/// for it before, or a new one. Returns its id, which names its key.
pub fn set_default(write: &mut Write, vendor: &str, model: &str) -> Result<Uuid, database::Error> {
    let existing: Option<Uuid> = write
        .prepare_device("SELECT id FROM llm_providers WHERE vendor = ?1 ORDER BY added_at LIMIT 1")?
        .query_row([vendor], |row| row.get(0))
        .optional()?;
    let provider = match existing {
        Some(provider) => provider,
        None => {
            let provider = Uuid::now_v7();
            let name = VENDORS
                .iter()
                .find(|(id, _)| *id == vendor)
                .map_or(vendor, |(_, name)| name);
            let now = write.now();
            write.insert(
                LlmProviders,
                provider,
                &[
                    llm_providers::NAME.value(&name),
                    llm_providers::VENDOR.value(&vendor),
                    llm_providers::ADDED_AT.value(&now),
                ],
            )?;
            provider
        }
    };
    let assigned: bool = write
        .prepare_device("SELECT EXISTS (SELECT 1 FROM llm_assignments WHERE task = 'default')")?
        .query_row([], |row| row.get(0))?;
    if assigned {
        write.update(
            LlmAssignments,
            "default",
            &[
                llm_assignments::PROVIDER_ID.value(&provider),
                llm_assignments::MODEL.value(&model),
            ],
        )?;
    } else {
        write.insert(
            LlmAssignments,
            "default",
            &[
                llm_assignments::PROVIDER_ID.value(&provider),
                llm_assignments::MODEL.value(&model),
            ],
        )?;
    }
    Ok(provider)
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

/// Sends `request` to the provider's model.
pub fn complete(
    provider: &Provider,
    request: CompletionRequest,
    cx: &App,
) -> Task<Result<Reply, Error>> {
    let Some(runtime) = cx.try_global::<Runtime>() else {
        return Task::ready(Err(Error::Runtime("no runtime".into())));
    };
    let handle = runtime.0.handle().clone();
    let provider = provider.clone();
    cx.background_spawn(async move {
        let key = key(provider.id)
            .map_err(Error::Keyring)?
            .ok_or(Error::NoKey)?;
        handle
            .spawn(async move { call(&provider, key, request).await })
            .await
            .map_err(|error| Error::Runtime(error.to_string()))?
    })
}

/// Sends `request` with `key`. Runs on a tokio runtime, as rig's HTTP client
/// needs.
pub async fn call(
    provider: &Provider,
    key: String,
    request: CompletionRequest,
) -> Result<Reply, Error> {
    let config = match provider.vendor.as_str() {
        "deepseek" => OpenAIConfig::with_key(&DEEPSEEK, key),
        vendor => return Err(Error::Unsupported(vendor.to_owned())),
    };
    let config = match &provider.base_url {
        Some(base_url) => config.with_base_url(base_url),
        None => config,
    };
    let model = config.client().completion(provider.model.clone());
    let response = tokio::time::timeout(TIMEOUT, model.call(request))
        .await
        .map_err(|_| Error::TimedOut)?
        .map_err(Error::Provider)?;
    let cost = response
        .usage
        .cost
        .as_ref()
        .map(|cost| cost.total)
        .or_else(|| {
            Catalog::builtin()
                .resolve(&format!("{}/{}", provider.vendor, provider.model))
                .and_then(|spec| spec.pricing)
                .and_then(|pricing| pricing.cost(&response.usage))
                .map(|cost| cost.total)
        });
    Ok(Reply { response, cost })
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
        .unwrap_or(&run.provider.model)
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
            llm_runs::PROVIDER.value(&run.provider.name),
            llm_runs::VENDOR.value(&run.provider.vendor),
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

    #[test]
    fn a_vendor_made_default_keeps_its_provider_and_takes_the_model() -> Result<(), database::Error>
    {
        let mut writer = Writer::open_in_memory()?;
        assert_eq!(assigned(writer.device(), Purpose::ExtractFacts)?, None);
        let (first, _) = writer.execute(|write| set_default(write, "deepseek", DEFAULT_MODEL))?;
        let (second, _) =
            writer.execute(|write| set_default(write, "deepseek", "deepseek-v4-pro"))?;
        assert_eq!(first, second, "the key stays named by the same provider");
        assert_eq!(
            assigned(writer.device(), Purpose::ExtractFacts)?,
            Some(Provider {
                id: first,
                name: "DeepSeek".to_owned(),
                vendor: "deepseek".to_owned(),
                base_url: None,
                model: "deepseek-v4-pro".to_owned(),
            })
        );
        Ok(())
    }

    #[test]
    fn a_failed_call_is_recorded_without_tokens_or_cost() -> Result<(), database::Error> {
        let mut writer = Writer::open_in_memory()?;
        let (provider, _) =
            writer.execute(|write| set_default(write, "deepseek", DEFAULT_MODEL))?;
        let provider = assigned(writer.device(), Purpose::ExtractFacts)?
            .filter(|assigned| assigned.id == provider);
        let Some(provider) = provider else {
            panic!("the provider just set");
        };
        writer.execute(|write| {
            let document = crate::model::commands::import_document(write, None, "Built things.")?;
            record(
                write,
                &Run {
                    purpose: Purpose::ExtractFacts,
                    provider: &provider,
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
                DEFAULT_MODEL.to_owned(),
                1500,
                0,
                0.,
                1
            )
        );
        Ok(())
    }
}
