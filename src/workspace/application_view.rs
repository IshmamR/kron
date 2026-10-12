use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeZone, Timelike, Utc};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::calendar::Date;
use gpui_kit::component::date_picker::{DatePicker, DatePickerState};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::radio::Radio;
use gpui_kit::component::resizable::{ResizableState, h_resizable, resizable_panel};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::time_field::TimePrecision;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::{
    Action, Anchor, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, FontWeight, Hsla,
    IntoElement, KeyBinding, Pixels, SharedString, Window, actions, div, prelude::*, px,
};

use super::parts::{caption, card, heading};
use crate::model::{
    EventId, InterviewKind, Job, JobEvent, JobEventKind, JobId, Library, Nudge, ResumeId, Stage,
    StepKind,
};

const KEY_CONTEXT: &str = "ApplicationView";
const FORM_CONTEXT: &str = "EventForm";
const TIMELINE_MAX_WIDTH: Pixels = px(720.);
const DETAILS_WIDTH: Pixels = px(340.);
const DETAILS_MIN_WIDTH: Pixels = px(280.);
const DETAILS_MAX_WIDTH: Pixels = px(560.);
const DOT_SIZE: Pixels = px(9.);

actions!(application_view, [SaveEvent, CancelEvent, UndoDelete]);

/// Opens the form to log an event of this kind.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = application_view, no_json)]
pub struct LogStep {
    pub step: StepKind,
}

pub fn init(cx: &mut App) {
    let input_in = format!("{FORM_CONTEXT} > Input");
    cx.bind_keys([
        KeyBinding::new("secondary-z", UndoDelete, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-enter", SaveEvent, Some(FORM_CONTEXT)),
        KeyBinding::new("escape", CancelEvent, Some(FORM_CONTEXT)),
        // The text boxes' own Esc does nothing, and Ctrl+Enter in the note
        // would add a line.
        KeyBinding::new("secondary-enter", SaveEvent, Some(&input_in)),
        KeyBinding::new("escape", CancelEvent, Some(&input_in)),
    ]);
}

pub enum ApplicationViewEvent {
    /// Show the résumé that was sent, in the Résumé tab.
    ShowResume(ResumeId),
}

/// The job view's Application tab: where the application stands, its events,
/// and the job itself.
pub struct ApplicationView {
    library: Entity<Library>,
    job: Option<JobId>,
    form: Option<EventForm>,
    date: Entity<DatePickerState>,
    with: Entity<InputState>,
    note: Entity<TextareaState>,
    /// The last event deleted, to undo.
    deleted: Option<JobEvent>,
    /// The details panel's width.
    layout: Entity<ResizableState>,
    /// Holds focus when nothing in the tab has it, so its keys and the job
    /// view's still arrive.
    focus_handle: FocusHandle,
}

/// What's being logged or edited. The text and date live in the view's
/// input states.
struct EventForm {
    editing: Option<EventId>,
    step: StepKind,
    interview_kind: InterviewKind,
    resume: Option<ResumeId>,
    /// The time the picker was given, and the exact time it stands for. The
    /// picker only shows minutes; left as it is, the exact time is kept, so
    /// an event logged just now sorts after one logged a moment before.
    prefilled: Option<(NaiveDateTime, DateTime<Utc>)>,
    /// Why the last save didn't happen.
    error: Option<&'static str>,
}

impl EventEmitter<ApplicationViewEvent> for ApplicationView {}

impl Render for ApplicationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cx: &Context<Self> = cx;
        let library = self.library.read(cx);
        let Some(job) = self.job.and_then(|job| library.job(job)) else {
            return div().into_any_element();
        };
        let now = Utc::now();
        let adding = self.form.as_ref().is_some_and(|form| form.editing.is_none());

        // Focus is tracked on this wrapper rather than the resizable layout,
        // which doesn't take it.
        div()
            .track_focus(&self.focus_handle)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, action: &LogStep, window, cx| {
                this.open_form(action.step, None, window, cx)
            }))
            .on_action(cx.listener(|this, _: &UndoDelete, window, cx| {
                this.undo_delete(window, cx)
            }))
            .size_full()
            .child(
                h_resizable("application")
                    .with_state(&self.layout)
                    .child(
                        resizable_panel().child(
                            div()
                                .id("application-scroll")
                                .size_full()
                                .overflow_y_scrollbar()
                                .child(
                                    v_flex()
                                        .max_w(TIMELINE_MAX_WIDTH)
                                        .p_4()
                                        .gap_5()
                                        .child(self.render_status(job, now, cx))
                                        .when(adding, |this| this.child(self.render_form(job, cx)))
                                        .child(self.render_timeline(job, now, cx)),
                                ),
                        ),
                    )
                    .child(
                        resizable_panel()
                            .size(DETAILS_WIDTH)
                            .size_range(DETAILS_MIN_WIDTH..DETAILS_MAX_WIDTH)
                            .child(self.render_details(job, cx)),
                    ),
            )
            .into_any_element()
    }
}

impl ApplicationView {
    pub fn new(library: Entity<Library>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            date: cx.new(|cx| {
                DatePickerState::new(window, cx)
                    .time_precision(TimePrecision::Minute)
                    .date_format("%b %-d, %Y  %H:%M")
            }),
            with: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Who you're meeting, and their role")
            }),
            note: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 8)
                    .placeholder("Anything worth remembering")
            }),
            library,
            job: None,
            form: None,
            deleted: None,
            layout: cx.new(|_| ResizableState::default()),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn set_job(&mut self, job: JobId, cx: &mut Context<Self>) {
        if self.job != Some(job) {
            self.job = Some(job);
            self.form = None;
            self.deleted = None;
            cx.notify();
        }
    }

    /// Opens the form for a new event of kind `step`, or to edit `existing`.
    fn open_form(
        &mut self,
        step: StepKind,
        existing: Option<JobEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let library = self.library.read(cx);
        let Some(job) = self.job.and_then(|job| library.job(job)) else {
            return;
        };
        let current_resume = job.current_resume().map(|resume| resume.id);
        let (interview_kind, with) = match existing.as_ref().map(|event| &event.kind) {
            Some(JobEventKind::Interview { kind, with }) => (*kind, with.clone()),
            _ => (InterviewKind::Phone, String::new()),
        };
        let resume = match existing.as_ref().map(|event| &event.kind) {
            Some(JobEventKind::Applied { resume }) => *resume,
            _ => current_resume,
        };
        // A new interview has no sensible default time; the user picks one.
        let exact = match (&existing, step) {
            (Some(event), _) => Some(event.at),
            (None, StepKind::Interview) => None,
            (None, _) => Some(Utc::now()),
        };
        let prefilled = exact.map(|exact| {
            let local = exact.with_timezone(&Local).naive_local();
            let minute = local
                .with_second(0)
                .and_then(|local| local.with_nanosecond(0))
                .unwrap_or(local);
            (minute, exact)
        });
        let note = existing
            .as_ref()
            .and_then(|event| event.note.clone())
            .unwrap_or_default();

        self.date.update(cx, |date, cx| match prefilled {
            Some((minute, _)) => date.set_date_time(minute, window, cx),
            None => date.set_date(Date::Single(None), window, cx),
        });
        self.with
            .update(cx, |input, cx| input.set_value(with, window, cx));
        self.note
            .update(cx, |input, cx| input.set_value(note, window, cx));
        match step {
            StepKind::Interview => self.with.update(cx, |input, cx| input.focus(window, cx)),
            _ => self.note.update(cx, |input, cx| input.focus(window, cx)),
        }
        self.form = Some(EventForm {
            editing: existing.map(|event| event.id),
            step,
            interview_kind,
            resume,
            prefilled,
            error: None,
        });
        cx.notify();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form = None;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(form), Some(job)) = (self.form.as_ref(), self.job) else {
            return;
        };
        let note = self.note.read(cx).value().trim().to_string();
        let note = (!note.is_empty()).then_some(note);
        let picked = self.date.read(cx).date_time().start();
        let error = match (form.step, picked, &note) {
            (_, None, _) if form.step == StepKind::Interview => Some("Pick when it's held."),
            (_, None, _) => Some("Pick when it happened."),
            (StepKind::Note, _, None) => Some("Write the note first."),
            _ => None,
        };
        if let Some(error) = error {
            if let Some(form) = self.form.as_mut() {
                form.error = Some(error);
            }
            cx.notify();
            return;
        }
        let at = match (picked, form.prefilled) {
            // Left as given: keep the exact time, or for a new event, now.
            (Some(picked), Some((minute, exact))) if picked == minute => match form.editing {
                Some(_) => exact,
                None => Utc::now(),
            },
            (picked, _) => picked
                .and_then(|naive| Local.from_local_datetime(&naive).earliest())
                .map_or_else(Utc::now, |local| local.with_timezone(&Utc)),
        };
        let kind = match form.step {
            StepKind::Applied => JobEventKind::Applied {
                resume: form.resume,
            },
            StepKind::RecruiterReplied => JobEventKind::RecruiterReplied,
            StepKind::Interview => JobEventKind::Interview {
                kind: form.interview_kind,
                with: self.with.read(cx).value().trim().to_string(),
            },
            StepKind::Offer => JobEventKind::Offer,
            StepKind::Rejected => JobEventKind::Rejected,
            StepKind::Withdrawn => JobEventKind::Withdrawn,
            StepKind::NoResponse => JobEventKind::NoResponse,
            StepKind::Note => JobEventKind::Note,
        };
        let event = JobEvent {
            id: form.editing.unwrap_or_else(EventId::new),
            at,
            kind,
            note,
        };
        self.form = None;
        self.deleted = None;
        self.library.update(cx, |library, cx| {
            library.save_event(job, event);
            cx.notify();
        });
        self.focus_handle.focus(window, cx);
    }

    fn delete(&mut self, event: EventId, cx: &mut Context<Self>) {
        let Some(job) = self.job else {
            return;
        };
        self.deleted = self.library.update(cx, |library, cx| {
            cx.notify();
            library.delete_event(job, event)
        });
        if self.form.as_ref().is_some_and(|form| form.editing == Some(event)) {
            self.form = None;
        }
    }

    fn undo_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(event), Some(job)) = (self.deleted.take(), self.job) else {
            return;
        };
        self.focus_handle.focus(window, cx);
        self.library.update(cx, |library, cx| {
            library.save_event(job, event);
            cx.notify();
        });
    }

    fn log_now(&mut self, kind: JobEventKind, cx: &mut Context<Self>) {
        let Some(job) = self.job else {
            return;
        };
        self.deleted = None;
        self.library.update(cx, |library, cx| {
            library.save_event(job, JobEvent::new(Utc::now(), kind));
            cx.notify();
        });
    }

    fn render_status(&self, job: &Job, now: DateTime<Utc>, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let stage = job.stage(now);
        let nudge = job.nudge(now);
        let headline = match nudge {
            Some(Nudge::InterviewSoon { at }) => interview_soon_label(at, now),
            _ => stage_label(stage, now),
        };
        let last_update = job
            .timeline()
            .into_iter()
            .find(|event| event.at <= now)
            .map(|event| event.at);
        let detail = match (stage, last_update) {
            (Stage::NotApplied, _) => "Log it when you apply; the résumé you send is kept.".into(),
            (_, Some(at)) => format!("Last update {}", date_label(at, false)),
            (_, None) => String::new(),
        };
        let applied = job
            .events
            .iter()
            .any(|event| matches!(event.kind, JobEventKind::Applied { .. }));
        let closed = stage.is_closed();

        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_start()
                    .gap_4()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(headline),
                            )
                            .child(caption(detail, cx)),
                    )
                    .child(
                        Button::new("log-event")
                            .label("Log")
                            .icon(IconName::Plus)
                            .outline()
                            .small()
                            // Always in the same order, so each item stays where
                            // the hand expects it; ones that don't fit are
                            // disabled rather than hidden.
                            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                                for step in StepKind::ALL {
                                    let disabled = match step {
                                        StepKind::Applied => applied,
                                        StepKind::Rejected
                                        | StepKind::Withdrawn
                                        | StepKind::NoResponse => closed,
                                        _ => false,
                                    };
                                    if step == StepKind::Note {
                                        menu = menu.separator();
                                    }
                                    menu = menu.menu_with_disabled(
                                        step_label(step),
                                        Box::new(LogStep { step }),
                                        disabled,
                                    );
                                }
                                menu
                            }),
                    ),
            )
            .when_some(
                nudge.and_then(|nudge| quiet_label(nudge, now)),
                |this, label| {
                    this.child(
                        h_flex()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(theme.border)
                            .text_sm()
                            .child(
                                Icon::new(IconName::Info)
                                    .small()
                                    .text_color(theme.muted_foreground),
                            )
                            .child(div().flex_1().child(label))
                            .child(
                                Button::new("no-response")
                                    .label("Mark as no response")
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Closes the application")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.log_now(JobEventKind::NoResponse, cx)
                                    })),
                            ),
                    )
                },
            )
            .when_some(self.deleted.as_ref(), |this, deleted| {
                this.child(
                    h_flex()
                        .gap_2()
                        .justify_between()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius)
                        .bg(theme.secondary)
                        .text_sm()
                        .child(format!("Deleted “{}”.", event_title(deleted, now)))
                        .child(
                            Button::new("undo-delete")
                                .label("Undo")
                                .icon(IconName::Undo2)
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.undo_delete(window, cx)
                                })),
                        ),
                )
            })
    }

    fn render_form(&self, job: &Job, cx: &Context<Self>) -> impl IntoElement {
        let Some(form) = self.form.as_ref() else {
            return div().into_any_element();
        };
        let title = match (form.editing, form.step) {
            (Some(_), step) => format!("Edit “{}”", step_label(step)),
            (None, StepKind::Note) => "Add a note".to_string(),
            (None, step) => format!("Log “{}”", step_label(step)),
        };
        let interview_kind_index = InterviewKind::ALL
            .iter()
            .position(|&kind| kind == form.interview_kind)
            .unwrap_or_default();
        let field = |label: &'static str, child: AnyElement| {
            v_flex().gap_1().child(caption(label, cx)).child(child)
        };

        let theme = cx.theme();
        card(cx)
            .key_context(FORM_CONTEXT)
            .on_action(cx.listener(|this, _: &SaveEvent, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &CancelEvent, window, cx| this.cancel(window, cx)))
            .p_3()
            .gap_3()
            .border_1()
            .border_color(theme.list_active_border)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(field(
                if form.step == StepKind::Interview {
                    "When it's held"
                } else {
                    "When"
                },
                div().w(px(260.)).child(DatePicker::new(&self.date)).into_any_element(),
            ))
            .when(form.step == StepKind::Interview, |this| {
                this.child(field(
                    "Kind",
                    TabBar::new("interview-kind")
                        .segmented()
                        .small()
                        .selected_index(interview_kind_index)
                        .children(
                            InterviewKind::ALL.map(|kind| Tab::new().label(interview_kind_label(kind))),
                        )
                        .on_click(cx.listener(|this, index: &usize, _, cx| {
                            if let (Some(form), Some(&kind)) =
                                (this.form.as_mut(), InterviewKind::ALL.get(*index))
                            {
                                form.interview_kind = kind;
                                cx.notify();
                            }
                        }))
                        .into_any_element(),
                ))
                .child(field("With", Input::new(&self.with).into_any_element()))
            })
            .when(form.step == StepKind::Applied && job.resumes.len() > 1, |this| {
                this.child(field(
                    "Résumé sent",
                    v_flex()
                        .gap_1()
                        .children(job.resumes.iter().rev().enumerate().map(|(index, resume)| {
                            let id = resume.id;
                            let current = job.current_resume().map(|resume| resume.id) == Some(id);
                            Radio::new(("sent-resume", index))
                                .label(format!(
                                    "Generated {}{}",
                                    date_label(resume.generated_at, true),
                                    if current { " (current)" } else { "" }
                                ))
                                .checked(form.resume == Some(id))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(form) = this.form.as_mut() {
                                        form.resume = Some(id);
                                        cx.notify();
                                    }
                                }))
                        }))
                        .into_any_element(),
                ))
            })
            .child(field(
                if form.step == StepKind::Note {
                    "Note"
                } else {
                    "Note (optional)"
                },
                Textarea::new(&self.note).into_any_element(),
            ))
            .when_some(form.error, |this, error| {
                this.child(div().text_sm().text_color(theme.danger).child(error))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("save-event")
                            .label("Save")
                            .primary()
                            .small()
                            .tooltip("Ctrl+↩")
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .child(
                        Button::new("cancel-event")
                            .label("Cancel")
                            .ghost()
                            .small()
                            .tooltip("Esc")
                            .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_timeline(&self, job: &Job, now: DateTime<Utc>, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let timeline = job.timeline();
        if timeline.is_empty() {
            return v_flex()
                .child(heading("Timeline", None, cx))
                .child(caption("Nothing logged yet.", cx))
                .into_any_element();
        }

        v_flex()
            .child(heading("Timeline", None, cx))
            .child(v_flex().children(timeline.into_iter().map(|event| {
                let called_off = job.is_called_off(event, now);
                let upcoming = event.at > now && !called_off;
                let id = event.id;
                let editing = self.form.as_ref().and_then(|form| form.editing) == Some(id);
                let editable = event.clone();
                let details = event_details(job, event);
                let group = SharedString::from(format!("event-{}", id.0));

                // A line down the left joins the dots. Not a border, which
                // would be drawn over them.
                div()
                    .id(group.clone())
                    .group(group.clone())
                    .relative()
                    .ml_1()
                    .pl_5()
                    .pb_4()
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(px(1.))
                            .bg(theme.border),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(-DOT_SIZE / 2.)
                            .top(px(5.))
                            .size(DOT_SIZE)
                            .rounded_full()
                            .bg(if called_off {
                                theme.muted_foreground
                            } else {
                                dot_color(&event.kind, upcoming, cx)
                            }),
                    )
                    // Edited in place, so it's clear which event the form is for.
                    .when(editing, |this| this.child(self.render_form(job, cx)))
                    .when(!editing, |this| this.child(
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(event_title(event, now)),
                                    )
                                    .when(upcoming, |this| {
                                        this.child(Tag::secondary().outline().xsmall().child("Upcoming"))
                                    })
                                    .when(called_off, |this| {
                                        this.child(
                                            Tag::secondary()
                                                .outline()
                                                .xsmall()
                                                .child("Called off"),
                                        )
                                    })
                                    .child(div().flex_1())
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .invisible()
                                            .group_hover(group.clone(), |style| style.visible())
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "edit-{}",
                                                    id.0
                                                )))
                                                .label("Edit")
                                                .ghost()
                                                .xsmall()
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        let step = StepKind::of(&editable.kind);
                                                        this.open_form(
                                                            step,
                                                            Some(editable.clone()),
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                )),
                                            )
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "delete-{}",
                                                    id.0
                                                )))
                                                .label("Delete")
                                                .ghost()
                                                .xsmall()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.delete(id, cx)
                                                })),
                                            ),
                                    )
                                    .child(caption(
                                        date_label(
                                            event.at,
                                            matches!(event.kind, JobEventKind::Interview { .. }),
                                        ),
                                        cx,
                                    )),
                            )
                            .when_some(details, |this, details| this.child(caption(details, cx)))
                            // The user's own words: the size of the details, in the
                            // text colour.
                            .when_some(event.note.clone(), |this, note| {
                                this.child(div().text_xs().child(note))
                            }),
                    ))
            })))
            .into_any_element()
    }

    fn render_details(&self, job: &Job, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let sent = job.sent_resume();
        let current = job.current_resume().map(|resume| resume.id);

        v_flex()
            .id("job-details")
            .size_full()
            .overflow_y_scrollbar()
            .p_3()
            .gap_5()
            .bg(theme.sidebar)
            .border_l_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .gap_1()
                    .child(heading("Job", None, cx))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(job.title.clone()),
                    )
                    .child(caption(job.company.clone(), cx))
                    .when_some(job.url.clone(), |this, url| {
                        this.child(
                            h_flex().child(
                                Button::new("job-url")
                                    .label("Open the posting")
                                    .icon(IconName::ExternalLink)
                                    .ghost()
                                    .xsmall()
                                    .tooltip(url.clone())
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            ),
                        )
                    })
                    .child(
                        div()
                            .pt_2()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(job.description.clone()),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(heading("Sent", None, cx))
                    .child(match sent {
                        Some(resume) => {
                            let id = resume.id;
                            let older = Some(id) != current;
                            card(cx)
                                .gap_1()
                                .child(div().text_sm().child(format!(
                                    "Résumé generated {}",
                                    date_label(resume.generated_at, true)
                                )))
                                .when(older, |this| {
                                    this.child(caption(
                                        "Older than the current version: you regenerated after \
                                         sending it.",
                                        cx,
                                    ))
                                })
                                .child(
                                    h_flex().child(
                                        Button::new("view-sent")
                                            .label("View")
                                            .ghost()
                                            .xsmall()
                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                cx.emit(ApplicationViewEvent::ShowResume(id))
                                            })),
                                    ),
                                )
                                .into_any_element()
                        }
                        None => caption(
                            "Nothing sent yet. Logging “Applied” records which résumé you sent.",
                            cx,
                        )
                        .into_any_element(),
                    }),
            )
    }
}

/// Where the application stands, in a few words, for headers. A nudge says
/// more than the stage, so it replaces it.
pub fn status_label(job: &Job, now: DateTime<Utc>) -> String {
    nudge_label(job, now).unwrap_or_else(|| stage_label(job.stage(now), now))
}

/// The application's nudge, short enough for a tag.
pub fn nudge_label(job: &Job, now: DateTime<Utc>) -> Option<String> {
    Some(match job.nudge(now)? {
        Nudge::InterviewSoon { at } => interview_soon_label(at, now),
        Nudge::NoReply { days } => format!("No reply in {days} days"),
        Nudge::NoNewsSinceInterview { days, .. } => format!("No news in {days} days"),
    })
}

pub fn stage_label(stage: Stage, now: DateTime<Utc>) -> String {
    match stage {
        Stage::NotApplied => "Not applied".into(),
        Stage::Applied => "Applied".into(),
        Stage::Replied => "Recruiter replied".into(),
        Stage::Interview { at, .. } if at > now => format!("Interview {}", date_label(at, false)),
        Stage::Interview { at, .. } => format!("Interviewed {}", date_label(at, false)),
        Stage::Offer => "Offer".into(),
        Stage::Rejected => "Rejected".into(),
        Stage::Withdrawn => "Withdrawn".into(),
        Stage::NoResponse => "No response".into(),
    }
}

fn interview_soon_label(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let local = at.with_timezone(&Local);
    let time = local.format("%H:%M");
    // Calendar days: an interview tomorrow morning is "tomorrow", however
    // few hours away.
    let days = (local.date_naive() - now.with_timezone(&Local).date_naive()).num_days();
    match days {
        ..=0 => format!("Interview today at {time}"),
        1 => format!("Interview tomorrow at {time}"),
        days => format!("Interview in {days} days"),
    }
}

/// The nudge for an application that's gone quiet.
fn quiet_label(nudge: Nudge, _now: DateTime<Utc>) -> Option<String> {
    match nudge {
        Nudge::NoReply { days } => Some(format!("No reply in {days} days")),
        Nudge::NoNewsSinceInterview { days, .. } => {
            Some(format!("No news in {days} days since the interview"))
        }
        Nudge::InterviewSoon { .. } => None,
    }
}

fn step_label(step: StepKind) -> &'static str {
    match step {
        StepKind::Applied => "Applied",
        StepKind::RecruiterReplied => "Recruiter replied",
        StepKind::Interview => "Interview",
        StepKind::Offer => "Offer",
        StepKind::Rejected => "Rejected",
        StepKind::Withdrawn => "Withdrew",
        StepKind::NoResponse => "No response",
        StepKind::Note => "Note",
    }
}

fn interview_kind_label(kind: InterviewKind) -> &'static str {
    match kind {
        InterviewKind::Phone => "Phone",
        InterviewKind::Technical => "Technical",
        InterviewKind::Onsite => "Onsite",
        InterviewKind::Other => "Other",
    }
}

fn event_title(event: &JobEvent, now: DateTime<Utc>) -> String {
    match &event.kind {
        JobEventKind::Interview { kind, .. } => {
            let kind = match kind {
                InterviewKind::Other => "Interview".to_string(),
                kind => format!("{} interview", interview_kind_label(*kind)),
            };
            if event.at > now {
                kind
            } else {
                format!("{kind} held")
            }
        }
        JobEventKind::NoResponse => "Closed: no response".into(),
        kind => step_label(StepKind::of(kind)).into(),
    }
}

fn event_details(job: &Job, event: &JobEvent) -> Option<String> {
    match &event.kind {
        JobEventKind::Interview { with, .. } if !with.is_empty() => Some(format!("With {with}")),
        JobEventKind::Applied {
            resume: Some(resume),
        } => job.resume(*resume).map(|resume| {
            format!(
                "Sent the résumé generated {}",
                date_label(resume.generated_at, true)
            )
        }),
        _ => None,
    }
}

/// "Oct 4", "Oct 4 at 14:00", with the year when it isn't this one.
fn date_label(at: DateTime<Utc>, with_time: bool) -> String {
    let local = at.with_timezone(&Local);
    let mut label = if local.year() == Local::now().year() {
        local.format("%b %-d").to_string()
    } else {
        local.format("%b %-d, %Y").to_string()
    };
    if with_time {
        label.push_str(&local.format(" at %H:%M").to_string());
    }
    label
}

fn dot_color(kind: &JobEventKind, upcoming: bool, cx: &App) -> Hsla {
    let theme = cx.theme();
    match kind {
        _ if upcoming => theme.blue,
        JobEventKind::Offer => theme.success,
        JobEventKind::Rejected | JobEventKind::Withdrawn | JobEventKind::NoResponse => {
            theme.muted_foreground
        }
        JobEventKind::Note => theme.border,
        _ => theme.foreground,
    }
}
