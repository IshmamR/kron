// The AI providers and the model each task runs on, in Settings. Several
// providers can be set up at once, each with its own key; a key is saved in the
// OS keyring and never shown again, so a provider's row only says whether one is
// saved.

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::searchable_list::{SearchableGroup, SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme, Disableable, IconName, Sizable, WindowExt, h_flex, v_flex};
use gpui_kit::{
    App, AppContext, Context, Div, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, Pixels,
    SharedString, Subscription, Task, Window, actions, div, prelude::*, px,
};
use uuid::Uuid;

use super::parts::text_field;
use crate::database::schema::{LlmAssignments, LlmProviders};
use crate::database::{Changed, Database};
use crate::llm::{self, Provider, Purpose, Vendor};

/// Wide enough for a key to show it's all there.
const FIELD_WIDTH: Pixels = px(380.);
/// Eight rows of vendor tiles; more scroll.
const VENDORS_HEIGHT: Pixels = px(440.);
/// Wide enough for a model's name and its provider's.
const PICKER_WIDTH: Pixels = px(280.);
/// What each model picker sets, in the order they're listed: the default,
/// then each task, which may use the default.
pub const TASKS: [Option<Purpose>; 3] = [
    None,
    Some(Purpose::ExtractFacts),
    Some(Purpose::GenerateResume),
];

/// The provider tiles in the dialog, while they have focus.
const TILES_CONTEXT: &str = "VendorTiles";
/// The tiles are laid out in this many columns.
const TILE_COLUMNS: isize = 2;

actions!(ai_settings, [PickLeft, PickRight, PickUp, PickDown]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("left", PickLeft, Some(TILES_CONTEXT)),
        KeyBinding::new("right", PickRight, Some(TILES_CONTEXT)),
        KeyBinding::new("up", PickUp, Some(TILES_CONTEXT)),
        KeyBinding::new("down", PickDown, Some(TILES_CONTEXT)),
    ]);
}

type Models = SearchableVec<SearchableGroup<ModelItem>>;

pub struct AiSettings {
    database: Entity<Database>,
    providers: Vec<Provider>,
    keys: HashMap<Uuid, KeyStatus>,
    /// The models each OpenAI-compatible server says it serves.
    /// By address, so a list fetched while adding a server isn't fetched
    /// again when the page rereads its providers.
    served: HashMap<String, Vec<String>>,
    /// One per entry of `TASKS`.
    pickers: Vec<Entity<SelectState<Models>>>,
    /// The vendor picked in the dialog to add.
    picked: Option<&'static Vendor>,
    /// The provider whose row asks before removing it.
    confirming: Option<Uuid>,
    /// The vendor tiles, so the arrow keys pick between them.
    tiles: FocusHandle,
    editor: Option<Editor>,
    name: Entity<InputState>,
    url: Entity<InputState>,
    key: Entity<InputState>,
    saving: Saving,
    /// Saving what the dialog holds, or removing a provider; replaced, the
    /// one before stops.
    task: Task<()>,
    /// Reading the keys from the keyring and the servers' models.
    reading: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl AiSettings {
    pub fn new(database: Entity<Database>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Paste your API key")
        });
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Name, like Ollama"));
        let url = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Address, like http://localhost:11434/v1")
        });
        let pickers: Vec<_> = TASKS
            .iter()
            .map(|_| {
                cx.new(|cx| {
                    SelectState::new(Models::new(Vec::new()), None, window, cx).searchable(true)
                })
            })
            .collect();
        let mut subscriptions = vec![cx.subscribe_in(
            &database,
            window,
            |this, _, changed: &Changed, window, cx| {
                if changed.touches(LlmProviders) || changed.touches(LlmAssignments) {
                    this.reload(window, cx);
                }
            },
        )];
        for input in [&key, &name, &url] {
            subscriptions.push(
                cx.subscribe_in(input, window, |this, _, event, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.save(window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }),
            );
        }
        for (picker, task) in pickers.iter().zip(TASKS) {
            subscriptions.push(cx.subscribe_in(
                picker,
                window,
                move |this, _, event: &SelectEvent<Models>, _, cx| {
                    let SelectEvent::Confirm(choice) = event;
                    // The default can't be cleared, only changed.
                    if task.is_none() && choice.is_none() {
                        return;
                    }
                    let choice = choice.clone();
                    this.database.read(cx).queue(move |write| {
                        llm::assign(
                            write,
                            task,
                            choice
                                .as_ref()
                                .map(|(provider, model)| (*provider, model.as_str())),
                        )
                    });
                },
            ));
        }
        let mut settings = Self {
            database,
            providers: Vec::new(),
            keys: HashMap::new(),
            served: HashMap::new(),
            pickers,
            picked: None,
            confirming: None,
            tiles: cx.focus_handle(),
            editor: None,
            name,
            url,
            key,
            saving: Saving::Idle,
            task: Task::ready(()),
            reading: Task::ready(()),
            _subscriptions: subscriptions,
        };
        settings.reload(window, cx);
        settings
    }

    /// The providers set up, oldest first.
    pub fn providers(&self) -> &[Provider] {
        &self.providers
    }

    /// The buttons to add a provider, while any vendor isn't set up, and to
    /// add a server.
    pub fn render_add(&mut self, cx: &mut Context<Self>) -> Div {
        let all_added = self.vendors_left().next().is_none();
        h_flex()
            .flex_wrap()
            .gap_2()
            .child(
                Button::new("add-provider")
                    .icon(IconName::Plus)
                    .label("Add provider")
                    .primary()
                    .small()
                    .disabled(all_added)
                    .when(all_added, |this| this.tooltip("Every provider is added."))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_editor(Editor::NewProvider, window, cx)
                    })),
            )
            .child(
                Button::new("add-server")
                    .icon(IconName::Plus)
                    .label("Add server")
                    .outline()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_editor(Editor::Server(None), window, cx)
                    })),
            )
    }

    /// A saved provider's row: whether its key is saved, and buttons to
    /// replace the key and remove the provider.
    pub fn render_provider(&mut self, provider: Provider, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let status = self.keys.get(&provider.id).unwrap_or(&KeyStatus::Checking);
        let working = matches!(status, KeyStatus::Checking | KeyStatus::Working);
        let (text, color) = match status {
            KeyStatus::Checking => ("Checking the keyring…".into(), theme.muted_foreground),
            KeyStatus::Working => ("Removing…".into(), theme.muted_foreground),
            KeyStatus::Saved => (SharedString::from("Key saved."), theme.muted_foreground),
            KeyStatus::Missing if provider.is_compatible() => (
                SharedString::from("No key: none is sent."),
                theme.muted_foreground,
            ),
            KeyStatus::Missing => (
                SharedString::from(match llm::vendor(&provider.vendor) {
                    Some(vendor) => format!("No key saved. Get one at {}.", vendor.keys_at),
                    None => "No key saved.".to_owned(),
                }),
                theme.danger,
            ),
            KeyStatus::Failed(message) => (message.clone(), theme.danger),
        };
        let saved = *status == KeyStatus::Saved;
        let id = provider.id;
        if self.confirming == Some(id) {
            return self.render_confirm_remove(&provider, cx);
        }
        h_flex()
            .flex_wrap()
            .justify_end()
            .gap_3()
            .child(div().text_xs().text_color(color).child(text))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(SharedString::from(format!("key-{id}")))
                            .label(match (provider.is_compatible(), saved) {
                                (true, _) => "Edit…",
                                (false, true) => "Replace key…",
                                (false, false) => "Add key…",
                            })
                            .outline()
                            .small()
                            .disabled(working)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let editor = match provider.is_compatible() {
                                    true => Editor::Server(Some(provider.clone())),
                                    false => Editor::Key(provider.clone()),
                                };
                                this.open_editor(editor, window, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("remove-{id}")))
                            .label("Remove")
                            .ghost()
                            .small()
                            .disabled(working)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.confirming = Some(id);
                                cx.notify();
                            })),
                    ),
            )
    }

    /// Asked before removing, in the row: the key goes from the keyring
    /// for good, and can't be shown to copy first.
    fn render_confirm_remove(&mut self, provider: &Provider, cx: &mut Context<Self>) -> Div {
        let id = provider.id;
        let question = match provider.is_compatible() {
            true => format!("Remove {}?", provider.name),
            false => format!("Remove {} and its key?", provider.name),
        };
        h_flex()
            .flex_wrap()
            .justify_end()
            .gap_3()
            .child(div().text_xs().child(question))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(SharedString::from(format!("keep-{id}")))
                            .label("Cancel")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirming = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("confirm-remove-{id}")))
                            .label("Remove")
                            // The palette's danger is a text color checked
                            // against the page; the kit's danger button tints
                            // its fill red too, which loses the contrast.
                            .outline()
                            .text_color(cx.theme().danger)
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.confirming = None;
                                this.remove(id, cx);
                            })),
                    ),
            )
    }

    /// The model picker for `TASKS[index]`.
    pub fn render_picker(&self, index: usize) -> impl IntoElement + use<> {
        let task = TASKS.get(index).copied().flatten();
        let placeholder = match (task, self.providers.is_empty()) {
            (Some(_), _) => "Same as the default",
            (None, true) => "Add a provider first",
            (None, false) => "Pick a model",
        };
        let picker = &self.pickers[index.min(self.pickers.len() - 1)];
        Select::new(picker)
            .id(("model", index))
            .w(PICKER_WIDTH)
            .menu_width(PICKER_WIDTH)
            .small()
            .placeholder(placeholder)
            .search_placeholder("Search models")
            .cleanable(task.is_some())
            .disabled(self.providers.is_empty())
    }

    /// Reads the providers and what each task runs on again, and the keys
    /// and served models of providers new since.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let device = self.database.read(cx).device_connection();
        let providers = llm::providers(device).unwrap_or_else(|error| {
            eprintln!("ai settings: couldn't read the providers: {error}");
            Vec::new()
        });
        let added: Vec<Provider> = providers
            .iter()
            .filter(|provider| !self.keys.contains_key(&provider.id))
            .cloned()
            .collect();
        self.keys
            .retain(|id, _| providers.iter().any(|provider| provider.id == *id));
        self.served.retain(|url, _| {
            providers
                .iter()
                .any(|provider| provider.base_url.as_ref() == Some(url))
        });
        self.providers = providers;
        self.refresh_pickers(window, cx);
        if !added.is_empty() {
            self.read(added, window, cx);
        }
        cx.notify();
    }

    /// Fills the pickers with the models on offer, each showing what its
    /// task is set to.
    fn refresh_pickers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let device = self.database.read(cx).device_connection();
        let assigned: Vec<Option<(Uuid, String)>> = TASKS
            .iter()
            .map(|&task| {
                llm::assignment(device, task).unwrap_or_else(|error| {
                    eprintln!("ai settings: couldn't read what a task runs on: {error}");
                    None
                })
            })
            .collect();
        for (picker, choice) in self.pickers.iter().zip(&assigned) {
            let models = Models::new(models(&self.providers, &self.served, &assigned));
            picker.update(cx, |picker, cx| {
                picker.set_items(models, window, cx);
                match choice {
                    Some(choice) => picker.set_selected_value(choice, window, cx),
                    None => picker.set_selected_index(None, window, cx),
                }
            });
        }
    }

    /// Looks in the keyring for each provider's key, then asks each
    /// compatible server which models it serves.
    fn read(&mut self, providers: Vec<Provider>, window: &mut Window, cx: &mut Context<Self>) {
        for provider in &providers {
            self.keys.insert(provider.id, KeyStatus::Checking);
        }
        let ids: Vec<Uuid> = providers.iter().map(|provider| provider.id).collect();
        let found = cx.background_spawn(async move {
            ids.into_iter()
                .map(|id| (id, llm::key(id)))
                .collect::<Vec<_>>()
        });
        self.reading = cx.spawn_in(window, async move |this, cx| {
            let found = found.await;
            let mut keys = HashMap::new();
            for (id, key) in found {
                let (status, key) = match key {
                    Ok(Some(key)) => (KeyStatus::Saved, Some(key)),
                    Ok(None) => (KeyStatus::Missing, None),
                    Err(error) => (
                        KeyStatus::Failed(format!("Couldn't read the keyring: {error}.").into()),
                        None,
                    ),
                };
                keys.insert(id, (status, key));
            }
            let mut listings = Vec::new();
            let updated = this.update(cx, |this, cx| {
                for provider in &providers {
                    let Some(base_url) = provider.base_url.clone() else {
                        continue;
                    };
                    if this.served.contains_key(&base_url) {
                        continue;
                    }
                    let key = keys.get(&provider.id).and_then(|(_, key)| key.clone());
                    let listing = llm::served_models(base_url.clone(), key, cx);
                    listings.push((base_url, listing));
                }
                for (id, (status, _)) in keys {
                    this.keys.insert(id, status);
                }
                cx.notify();
            });
            if let Err(error) = updated {
                eprintln!("ai settings: {error}");
                return;
            }
            for (base_url, listing) in listings {
                let models = match listing.await {
                    Ok(models) => models,
                    Err(error) => {
                        eprintln!("ai settings: couldn't list a server's models: {error}");
                        continue;
                    }
                };
                if let Err(error) = this.update_in(cx, |this, window, cx| {
                    this.served.insert(base_url, models);
                    this.refresh_pickers(window, cx);
                    cx.notify();
                }) {
                    eprintln!("ai settings: {error}");
                }
            }
        });
    }

    fn open_editor(&mut self, editor: Editor, window: &mut Window, cx: &mut Context<Self>) {
        let title: SharedString = match &editor {
            Editor::NewProvider => "Add a provider".into(),
            Editor::Key(provider) => format!("{} API key", provider.name).into(),
            Editor::Server(None) => "OpenAI-compatible server".into(),
            Editor::Server(Some(server)) => format!("Edit {}", server.name).into(),
        };
        let placeholder = match &editor {
            Editor::Server(Some(server))
                if self.keys.get(&server.id) == Some(&KeyStatus::Saved) =>
            {
                "Leave empty to keep the saved key"
            }
            Editor::Server(_) => "API key, if it takes one",
            _ => "Paste your API key",
        };
        self.key
            .update(cx, |key, cx| key.set_placeholder(placeholder, window, cx));
        if let Editor::Server(Some(server)) = &editor {
            let url = server.base_url.clone().unwrap_or_default();
            self.name.update(cx, |name, cx| {
                name.set_value(server.name.clone(), window, cx)
            });
            self.url
                .update(cx, |input, cx| input.set_value(url, window, cx));
        }
        let first = match editor {
            Editor::Server(_) => self.name.read(cx).focus_handle(cx),
            Editor::NewProvider => self.tiles.clone(),
            Editor::Key(_) => self.key.read(cx).focus_handle(cx),
        };
        let vendor = self.vendors_left().next();
        self.picked = vendor;
        self.editor = Some(editor);
        self.saving = Saving::Idle;
        let settings = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let content = settings.clone();
            let closed = settings.clone();
            let confirmed = settings.clone();
            dialog
                .title(title.clone())
                .w(FIELD_WIDTH + px(48.))
                // Enter saves, and the dialog stays open until it's saved:
                // closing it stops the save.
                .on_ok(move |_, window, cx| {
                    confirmed.update(cx, |this, cx| this.save(window, cx));
                    false
                })
                .on_close(move |_, window, cx| {
                    closed.update(cx, |this, cx| this.close_editor(window, cx))
                })
                .content(move |dialog, window, cx| {
                    dialog.child(content.update(cx, |this, cx| this.render_editor(window, cx)))
                })
        });
        first.focus(window, cx);
        cx.notify();
    }

    fn render_editor(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let server = matches!(self.editor, Some(Editor::Server(_)));
        let new_provider = matches!(self.editor, Some(Editor::NewProvider));
        let working = self.saving == Saving::Working;
        let filled = |input: &Entity<InputState>| !input.read(cx).value().trim().is_empty();
        let ready = match server {
            true => filled(&self.name) && filled(&self.url),
            false => filled(&self.key) && (!new_provider || self.picked.is_some()),
        };
        let failed = match &self.saving {
            Saving::Failed(message) => Some(message.clone()),
            _ => None,
        };
        let vendors = new_provider.then(|| self.render_vendors(window, cx));
        let theme = cx.theme();
        v_flex()
            .gap_4()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(match server {
                        true => {
                            "Ollama, LM Studio, or any server with OpenAI's API. Its models \
                             are listed from it."
                        }
                        false => "Saved in your system keyring. The key is never shown again.",
                    }),
            )
            .children(vendors)
            .when(server, |this| {
                this.child(text_field(Input::new(&self.name), &self.name, cx))
                    .child(text_field(Input::new(&self.url), &self.url, cx))
            })
            .child(text_field(
                Input::new(&self.key).mask_toggle(),
                &self.key,
                cx,
            ))
            .when_some(failed, |this, message| {
                this.child(div().text_xs().text_color(theme.danger).child(message))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-editor")
                            .label("Cancel")
                            .outline()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_editor(window, cx);
                                window.close_dialog(cx);
                            })),
                    )
                    .child(
                        Button::new("save-editor")
                            .label(match (working, server) {
                                (true, true) => "Connecting…",
                                (true, false) => "Saving…",
                                (false, _) => "Save",
                            })
                            .primary()
                            .small()
                            .disabled(working || !ready)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }

    /// Forgets what was typed, so a key isn't kept in memory.
    fn close_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor = None;
        self.task = Task::ready(());
        self.saving = Saving::Idle;
        for input in [&self.key, &self.name, &self.url] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    /// Saves what the dialog holds: a vendor's key, adding its provider if
    /// it's new, or a server, once it has listed its models.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else {
            return;
        };
        if self.saving == Saving::Working {
            return;
        }
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let (key, name, url) = (value(&self.key), value(&self.name), value(&self.url));
        // Enter on the tiles goes on to the key.
        if matches!(editor, Editor::NewProvider) && key.is_empty() {
            self.key.update(cx, |key, cx| key.focus(window, cx));
            return;
        }
        let database = self.database.clone();
        let saved = match (editor, self.picked) {
            (Editor::NewProvider, Some(vendor)) if !key.is_empty() => {
                let added = database.read(cx).write(cx, move |write| {
                    llm::add_provider(write, vendor.name, vendor.id, None, vendor.model)
                });
                cx.spawn(async move |_, cx| {
                    let provider = added
                        .await
                        .map_err(|error| format!("Couldn't save the provider: {error}."))?;
                    save_key(provider, key, true, &database, cx).await?;
                    Ok((provider, false))
                })
            }
            (Editor::Key(provider), _) if !key.is_empty() => cx.spawn(async move |_, cx| {
                save_key(provider.id, key, false, &database, cx).await?;
                Ok((provider.id, false))
            }),
            (Editor::Server(Some(server)), _) if !name.is_empty() && !url.is_empty() => {
                let id = server.id;
                let typed = (!key.is_empty()).then_some(key);
                cx.spawn(async move |this, cx| {
                    // Asked again even with the address unchanged, so a
                    // mistake shows here rather than when a task runs.
                    let listing_key = match typed.clone() {
                        Some(key) => Some(key),
                        None => cx
                            .background_spawn(async move { llm::key(id) })
                            .await
                            .map_err(|error| format!("Couldn't read the keyring: {error}."))?,
                    };
                    let listing = cx.update(|cx| llm::served_models(url.clone(), listing_key, cx));
                    let models = listing
                        .await
                        .map_err(|error| format!("Couldn't reach the server. {error}"))?;
                    let served = models.clone();
                    this.update(cx, |this, _| this.served.insert(url.clone(), models))
                        .map_err(|error| error.to_string())?;
                    let edited = cx.update(|cx| {
                        database.read(cx).write(cx, move |write| {
                            llm::edit_server(write, id, &name, &url, &served)
                        })
                    });
                    edited
                        .await
                        .map_err(|error| format!("Couldn't save the server: {error}."))?;
                    if let Some(key) = typed {
                        save_key(id, key, false, &database, cx).await?;
                    }
                    Ok((id, true))
                })
            }
            (Editor::Server(None), _) if !name.is_empty() && !url.is_empty() => {
                let key = (!key.is_empty()).then_some(key);
                let listing = llm::served_models(url.clone(), key.clone(), cx);
                cx.spawn(async move |this, cx| {
                    let models = listing
                        .await
                        .map_err(|error| format!("Couldn't reach the server. {error}"))?;
                    let Some(first) = models.first().cloned() else {
                        return Err("The server lists no models.".to_owned());
                    };
                    // Kept before it's saved, for the reread saving it sets off.
                    this.update(cx, |this, _| this.served.insert(url.clone(), models))
                        .map_err(|error| error.to_string())?;
                    let added = cx.update(|cx| {
                        database.read(cx).write(cx, move |write| {
                            llm::add_compatible(write, &name, &url, &first)
                        })
                    });
                    let provider = added
                        .await
                        .map_err(|error| format!("Couldn't save the server: {error}."))?;
                    if let Some(key) = key {
                        save_key(provider, key, true, &database, cx).await?;
                    }
                    Ok((provider, true))
                })
            }
            _ => return,
        };
        self.saving = Saving::Working;
        self.task = cx.spawn_in(window, async move |this, cx| {
            let saved = saved.await;
            let updated = this.update_in(cx, |this, window, cx| {
                match saved {
                    Ok((provider, listed)) => {
                        // An empty key field adds none, and keeps the one an
                        // edited server has.
                        match this.key.read(cx).value().trim().is_empty() {
                            true => {
                                this.keys.entry(provider).or_insert(KeyStatus::Missing);
                            }
                            false => {
                                this.keys.insert(provider, KeyStatus::Saved);
                            }
                        }
                        if listed {
                            this.refresh_pickers(window, cx);
                        }
                        this.close_editor(window, cx);
                        window.defer(cx, |window, cx| window.close_dialog(cx));
                    }
                    Err(message) => this.saving = Saving::Failed(message.into()),
                }
                cx.notify();
            });
            if let Err(error) = updated {
                eprintln!("ai settings: {error}");
            }
        });
        cx.notify();
    }

    /// The vendors Kron offers that aren't set up yet.
    fn vendors_left(&self) -> impl Iterator<Item = &'static Vendor> + '_ {
        llm::VENDORS.iter().filter(|vendor| {
            !self
                .providers
                .iter()
                .any(|provider| provider.vendor == vendor.id && !provider.is_compatible())
        })
    }

    /// The vendors to pick from, as tiles in the dialog rather than a
    /// dropdown, which would hang out of it. Two columns keep a long list
    /// short, and past `VENDORS_HEIGHT` it scrolls.
    fn render_vendors(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let focused = self.tiles.is_focused(window);
        let theme = cx.theme();
        div()
            .id("vendors")
            .track_focus(&self.tiles)
            .key_context(TILES_CONTEXT)
            .on_action(cx.listener(|this, _: &PickLeft, _, cx| this.move_pick(-1, cx)))
            .on_action(cx.listener(|this, _: &PickRight, _, cx| this.move_pick(1, cx)))
            .on_action(cx.listener(|this, _: &PickUp, _, cx| this.move_pick(-TILE_COLUMNS, cx)))
            .on_action(cx.listener(|this, _: &PickDown, _, cx| this.move_pick(TILE_COLUMNS, cx)))
            .max_h(VENDORS_HEIGHT)
            .overflow_y_scroll()
            .child(
                div()
                    .grid()
                    .grid_cols(TILE_COLUMNS as u16)
                    .gap_2()
                    .children(self.vendors_left().map(|vendor| {
                        let picked = self.picked.is_some_and(|picked| picked.id == vendor.id);
                        v_flex()
                            .id(SharedString::from(format!("vendor-{}", vendor.id)))
                            .min_w_0()
                            .px_3()
                            .py_2()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(match (picked, focused) {
                                // The keys move this one: the ring says so.
                                (true, true) => theme.ring,
                                (true, false) => theme.list_active_border,
                                (false, _) => theme.border,
                            })
                            .when(picked, |this| this.bg(theme.list_active))
                            .when(!picked, |this| this.hover(|this| this.bg(theme.list_hover)))
                            .cursor_pointer()
                            .child(div().text_sm().truncate().child(vendor.name))
                            .child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(theme.muted_foreground)
                                    .child(vendor.keys_at),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.picked = Some(vendor);
                                this.key.update(cx, |key, cx| key.focus(window, cx));
                                cx.notify();
                            }))
                    })),
            )
    }

    /// Picks the tile `step` away, across then down, staying on the grid.
    fn move_pick(&mut self, step: isize, cx: &mut Context<Self>) {
        let vendors: Vec<&'static Vendor> = self.vendors_left().collect();
        let Some(last) = vendors.len().checked_sub(1) else {
            return;
        };
        let current = self
            .picked
            .and_then(|picked| vendors.iter().position(|vendor| vendor.id == picked.id))
            .unwrap_or(0);
        let next = current as isize + step;
        if (0..=last as isize).contains(&next) {
            self.picked = vendors.get(next as usize).copied();
            cx.notify();
        }
    }

    /// Removes the provider's key, then the provider: the other way, a key
    /// the keyring kept would be left with nothing naming it.
    fn remove(&mut self, provider: Uuid, cx: &mut Context<Self>) {
        self.keys.insert(provider, KeyStatus::Working);
        let removed = cx.background_spawn(async move { llm::set_key(provider, None) });
        let database = self.database.clone();
        self.task = cx.spawn(async move |this, cx| {
            let failed = match removed.await {
                Ok(()) => {
                    let removed = cx.update(|cx| {
                        database
                            .read(cx)
                            .write(cx, move |write| llm::remove_provider(write, provider))
                    });
                    removed
                        .await
                        .err()
                        .map(|error| format!("Couldn't remove it: {error}."))
                }
                Err(error) => Some(format!(
                    "Couldn't remove the key from the keyring: {error}."
                )),
            };
            if let Some(message) = failed
                && let Err(error) = this.update(cx, |this, cx| {
                    this.keys
                        .insert(provider, KeyStatus::Failed(message.into()));
                    cx.notify();
                })
            {
                eprintln!("ai settings: {error}");
            }
        });
        cx.notify();
    }
}

/// What the dialog is open for.
#[derive(Clone)]
enum Editor {
    /// A vendor not set up yet, picked in the dialog, with its key: saving
    /// adds the provider.
    NewProvider,
    /// A new key for a saved provider.
    Key(Provider),
    /// An OpenAI-compatible server: a new one, or one saved, to edit.
    Server(Option<Provider>),
}

#[derive(Clone, Debug, PartialEq)]
enum KeyStatus {
    Checking,
    Saved,
    Missing,
    Working,
    Failed(SharedString),
}

#[derive(Clone, Debug, PartialEq)]
enum Saving {
    Idle,
    Working,
    Failed(SharedString),
}

/// A model in a picker, under its provider's name.
#[derive(Clone)]
struct ModelItem {
    name: SharedString,
    provider: SharedString,
    choice: (Uuid, String),
}

impl SearchableListItem for ModelItem {
    type Value = (Uuid, String);

    /// With the provider, since the closed picker shows no group.
    fn title(&self) -> SharedString {
        format!("{} · {}", self.name, self.provider).into()
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.name.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.choice
    }

    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        [self.name.as_ref(), self.provider.as_ref(), &self.choice.1]
            .iter()
            .any(|text| text.to_lowercase().contains(&query))
    }
}

/// The models a picker offers, grouped by provider: the catalog's for a
/// vendor, what a server listed for it, and any model a task is set to that
/// neither lists, so it still shows.
fn models(
    providers: &[Provider],
    served: &HashMap<String, Vec<String>>,
    assigned: &[Option<(Uuid, String)>],
) -> Vec<SearchableGroup<ModelItem>> {
    providers
        .iter()
        .map(|provider| {
            let mut listed: Vec<(String, String)> = match provider.is_compatible() {
                true => provider
                    .base_url
                    .as_ref()
                    .and_then(|url| served.get(url))
                    .into_iter()
                    .flatten()
                    .map(|model| (model.clone(), model.clone()))
                    .collect(),
                false => llm::models(&provider.vendor),
            };
            for (_, model) in assigned
                .iter()
                .flatten()
                .filter(|(id, _)| *id == provider.id)
            {
                if !listed.iter().any(|(id, _)| id == model) {
                    listed.push((model.clone(), model.clone()));
                }
            }
            let name = SharedString::from(provider.name.clone());
            SearchableGroup::new(name.clone()).items(listed.into_iter().map(|(model, display)| {
                ModelItem {
                    name: display.into(),
                    provider: name.clone(),
                    choice: (provider.id, model),
                }
            }))
        })
        .collect()
}

/// Saves `key` for the provider. If the keyring refuses it, a provider
/// just `added` is taken back out, so none is left without the key it was
/// added with.
async fn save_key(
    provider: Uuid,
    key: String,
    added: bool,
    database: &Entity<Database>,
    cx: &mut gpui_kit::AsyncApp,
) -> Result<(), String> {
    let saved = cx
        .background_spawn(async move { llm::set_key(provider, Some(&key)) })
        .await;
    let Err(error) = saved else {
        return Ok(());
    };
    if added {
        let removed = cx.update(|cx| {
            database
                .read(cx)
                .write(cx, move |write| llm::remove_provider(write, provider))
        });
        if let Err(error) = removed.await {
            eprintln!("ai settings: couldn't take back a provider without its key: {error}");
        }
    }
    Err(format!("Couldn't save to the keyring: {error}."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::TemporaryDirectory;
    use crate::workspace::testing;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{TestAppContext, VisualTestContext};

    /// The rows the settings page draws, drawn on their own.
    struct Rows(Entity<AiSettings>);

    impl Render for Rows {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |settings, cx| {
                let providers = settings.providers.clone();
                v_flex()
                    .children(
                        providers
                            .into_iter()
                            .map(|provider| settings.render_provider(provider, cx)),
                    )
                    .child(settings.render_add(cx))
                    .child(settings.render_picker(1))
            })
        }
    }

    /// Opens the rows on an empty database: with no provider, the keyring
    /// isn't read.
    fn open(
        app: &mut TestAppContext,
    ) -> (Entity<AiSettings>, VisualTestContext, TemporaryDirectory) {
        let (database, directory) = testing::open_empty(app);
        let (handle, rows) = app
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    let settings = cx.new(|cx| AiSettings::new(database, window, cx));
                    cx.new(|_| Rows(settings))
                })
            })
            .unwrap_or_else(|error| panic!("couldn't open the AI settings: {error}"));
        let cx = VisualTestContext::from_window(handle, app);
        let settings = rows.read_with(&cx, |rows, _| rows.0.clone());
        cx.run_until_parked();
        (settings, cx, directory)
    }

    fn typed(settings: &Entity<AiSettings>, cx: &VisualTestContext) -> String {
        settings.read_with(cx, |settings, cx| settings.key.read(cx).value().to_string())
    }

    #[test]
    fn cancelling_the_key_dialog_discards_the_draft() {
        let mut app = TestAppContext::single();
        let (settings, mut cx, _directory) = open(&mut app);
        cx.update(|window, cx| window.click("add-provider", cx));
        cx.run_until_parked();
        cx.update(|window, _| assert!(window.find("save-editor").visible()));
        // The tiles have focus first; Enter goes on to the key.
        cx.simulate_keystrokes("enter");
        cx.simulate_input("sk-not-a-real-key");
        cx.run_until_parked();
        assert_eq!(typed(&settings, &cx), "sk-not-a-real-key");
        cx.update(|window, cx| window.click("cancel-editor", cx));
        cx.run_until_parked();
        assert!(typed(&settings, &cx).is_empty());
        cx.update(|window, cx| window.click("add-provider", cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.simulate_input("sk-another-draft");
        assert_eq!(typed(&settings, &cx), "sk-another-draft");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(typed(&settings, &cx).is_empty());
    }

    #[test]
    fn a_server_that_cant_be_reached_isnt_saved_and_the_dialog_says_so() {
        let mut app = TestAppContext::single();
        let (settings, mut cx, _directory) = open(&mut app);
        cx.update(|window, cx| window.click("add-server", cx));
        cx.run_until_parked();
        cx.simulate_input("Ollama");
        cx.update(|window, cx| {
            settings.update(cx, |settings, cx| {
                settings.url.update(cx, |url, cx| {
                    url.set_value("http://localhost:1/v1", window, cx)
                })
            })
        });
        // Enter saves as the button does, without closing the dialog first:
        // closing it stops the save.
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        // The test app runs no runtime for calls, so the server can't be
        // asked: as when it's not there.
        let (saving, providers) = settings.read_with(&cx, |settings, _| {
            (settings.saving.clone(), settings.providers.len())
        });
        assert!(
            matches!(&saving, Saving::Failed(message) if message.starts_with("Couldn't reach")),
            "{saving:?}"
        );
        assert_eq!(providers, 0);
        cx.update(|window, _| assert!(window.find("save-editor").visible(), "the dialog stays"));
    }

    #[test]
    fn the_dialog_offers_only_the_providers_not_added_yet() {
        let mut app = TestAppContext::single();
        let (settings, mut cx, _directory) = open(&mut app);
        let added = |vendor: &Vendor| Provider {
            id: Uuid::now_v7(),
            name: vendor.name.to_owned(),
            vendor: vendor.id.to_owned(),
            format: None,
            base_url: None,
        };
        // Set without the database, so the keyring isn't read.
        settings.update(&mut cx, |settings, _| {
            settings.providers = llm::VENDORS[1..].iter().map(added).collect()
        });
        cx.update(|window, cx| window.click("add-provider", cx));
        cx.run_until_parked();
        let offered = settings.read_with(&cx, |settings, _| {
            (
                settings.picked.map(|vendor| vendor.id),
                settings
                    .vendors_left()
                    .map(|vendor| vendor.id)
                    .collect::<Vec<_>>(),
            )
        });
        assert_eq!(
            offered,
            (Some(llm::VENDORS[0].id), vec![llm::VENDORS[0].id])
        );
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        settings.update(&mut cx, |settings, cx| {
            settings.providers = llm::VENDORS.iter().map(added).collect();
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click("add-provider", cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("save-editor").is_none(),
                "with every vendor added, the button does nothing"
            );
        });
    }

    #[test]
    fn the_arrow_keys_pick_a_tile_and_enter_goes_on_to_the_key() {
        let mut app = TestAppContext::single();
        app.update(init);
        let (settings, mut cx, _directory) = open(&mut app);
        cx.update(|window, cx| window.click("add-provider", cx));
        cx.run_until_parked();
        let picked = |cx: &VisualTestContext| {
            settings.read_with(cx, |settings, _| settings.picked.map(|vendor| vendor.id))
        };
        let id = |index: usize| Some(llm::VENDORS[index].id);
        assert_eq!(picked(&cx), id(0));
        for (keys, expected) in [
            ("right", 1),
            ("down", 3),
            ("left", 2),
            ("up", 0),
            ("up", 0),
            ("left", 0),
        ] {
            cx.simulate_keystrokes(keys);
            assert_eq!(picked(&cx), id(expected), "after {keys}");
        }
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let key = settings.read(cx).key.read(cx).focus_handle(cx);
            assert!(key.is_focused(window), "Enter goes on to the key");
            assert!(
                window.find("save-editor").visible(),
                "and keeps the dialog open"
            );
        });
    }

    #[test]
    fn a_server_opens_to_edit_with_its_name_and_address() {
        let mut app = TestAppContext::single();
        let (settings, mut cx, _directory) = open(&mut app);
        let id = Uuid::now_v7();
        // Set without the database, so the keyring isn't read.
        settings.update(&mut cx, |settings, cx| {
            settings.providers = vec![Provider {
                id,
                name: "Ollama".to_owned(),
                vendor: "openai".to_owned(),
                format: Some("chat".to_owned()),
                base_url: Some("http://localhost:11434/v1".to_owned()),
            }];
            settings.keys.insert(id, KeyStatus::Missing);
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(SharedString::from(format!("key-{id}")), cx);
        });
        cx.run_until_parked();
        let fields = settings.read_with(&cx, |settings, cx| {
            (
                settings.name.read(cx).value().to_string(),
                settings.url.read(cx).value().to_string(),
            )
        });
        assert_eq!(
            fields,
            ("Ollama".to_owned(), "http://localhost:11434/v1".to_owned())
        );
        // A typed key, so saving doesn't read the keyring; the test app runs
        // no runtime for calls, so the server can't be asked.
        cx.update(|window, cx| {
            settings.update(cx, |settings, cx| {
                settings
                    .key
                    .update(cx, |key, cx| key.set_value("sk-local", window, cx))
            })
        });
        cx.update(|window, cx| window.click("save-editor", cx));
        cx.run_until_parked();
        let saving = settings.read_with(&cx, |settings, _| settings.saving.clone());
        assert!(
            matches!(&saving, Saving::Failed(message) if message.starts_with("Couldn't reach")),
            "{saving:?}"
        );
    }

    #[test]
    fn removing_a_provider_asks_first() {
        let mut app = TestAppContext::single();
        let (settings, mut cx, _directory) = open(&mut app);
        let id = Uuid::now_v7();
        // Set without the database, so the keyring isn't read.
        settings.update(&mut cx, |settings, cx| {
            settings.providers = vec![Provider {
                id,
                name: "DeepSeek".to_owned(),
                vendor: "deepseek".to_owned(),
                format: None,
                base_url: None,
            }];
            settings.keys.insert(id, KeyStatus::Saved);
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(SharedString::from(format!("remove-{id}")), cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .find(SharedString::from(format!("confirm-remove-{id}")))
                    .visible()
            );
            window.click(SharedString::from(format!("keep-{id}")), cx);
        });
        cx.run_until_parked();
        let (confirming, status) = settings.read_with(&cx, |settings, _| {
            (settings.confirming, settings.keys.get(&id).cloned())
        });
        assert_eq!(confirming, None);
        assert_eq!(status, Some(KeyStatus::Saved), "nothing was removed");
    }

    #[test]
    fn the_pickers_offer_each_providers_models_and_what_tasks_are_set_to() {
        let vendor = Provider {
            id: Uuid::now_v7(),
            name: "DeepSeek".to_owned(),
            vendor: "deepseek".to_owned(),
            format: None,
            base_url: None,
        };
        let server = Provider {
            id: Uuid::now_v7(),
            name: "Ollama".to_owned(),
            vendor: "openai".to_owned(),
            format: Some("chat".to_owned()),
            base_url: Some("http://localhost:11434/v1".to_owned()),
        };
        let served = HashMap::from([(
            "http://localhost:11434/v1".to_owned(),
            vec!["qwen3".to_owned()],
        )]);
        // A model the catalog dropped since it was picked.
        let assigned = [Some((vendor.id, "deepseek-chat".to_owned())), None, None];
        let groups = models(&[vendor.clone(), server.clone()], &served, &assigned);
        let listed = |group: &SearchableGroup<ModelItem>| {
            group
                .items
                .iter()
                .map(|item| item.choice.1.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            groups
                .iter()
                .map(|group| group.title.clone())
                .collect::<Vec<_>>(),
            vec!["DeepSeek", "Ollama"]
        );
        assert_eq!(
            listed(&groups[0]),
            vec!["deepseek-flash", "deepseek-v4-pro", "deepseek-chat"]
        );
        assert_eq!(listed(&groups[1]), vec!["qwen3"]);
        assert!(
            groups[1].items[0].matches("ollama"),
            "a search finds a provider's models"
        );
    }
}
