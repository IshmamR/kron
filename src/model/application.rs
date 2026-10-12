// Where an application stands: its events, and what follows from them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{EventId, Job, JobId, Library, ResumeId, TailoredResume};

/// After this long with no answer to an application, it's worth a follow-up,
/// or closing it.
const NO_REPLY_DAYS: i64 = 14;
/// After an interview, news usually comes sooner.
const NO_NEWS_AFTER_INTERVIEW_DAYS: i64 = 7;

/// Something that happened, or will, in an application.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobEvent {
    pub id: EventId,
    /// When it happened; for an interview, when it's held.
    pub at: DateTime<Utc>,
    pub kind: JobEventKind,
    /// The user's own words about it. A `Note` event is only this.
    pub note: Option<String>,
}

impl JobEvent {
    pub fn new(at: DateTime<Utc>, kind: JobEventKind) -> Self {
        Self {
            id: EventId::new(),
            at,
            kind,
            note: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobEventKind {
    /// `resume` is the version sent, which regenerating doesn't change.
    Applied { resume: Option<ResumeId> },
    RecruiterReplied,
    Interview { kind: InterviewKind, with: String },
    Offer,
    Rejected,
    Withdrawn,
    /// The user gave up waiting.
    NoResponse,
    Note,
}

impl JobEventKind {
    /// Whether it moves the application along; notes don't.
    pub fn is_step(&self) -> bool {
        !matches!(self, JobEventKind::Note)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterviewKind {
    Phone,
    Technical,
    Onsite,
    Other,
}

impl InterviewKind {
    pub const ALL: [Self; 4] = [Self::Phone, Self::Technical, Self::Onsite, Self::Other];
}

/// Where an application stands, from its latest step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    NotApplied,
    Applied,
    Replied,
    /// An interview, held or to come.
    Interview { at: DateTime<Utc>, kind: InterviewKind },
    Offer,
    Rejected,
    Withdrawn,
    NoResponse,
}

impl Stage {
    pub fn is_closed(self) -> bool {
        matches!(self, Stage::Rejected | Stage::Withdrawn | Stage::NoResponse)
    }

    pub fn is_interviewing(self) -> bool {
        matches!(self, Stage::Interview { .. })
    }
}

/// Something about an application worth the user's attention now, worked out
/// from its dates rather than stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nudge {
    InterviewSoon { at: DateTime<Utc> },
    /// Applied or heard back, then nothing for a while.
    NoReply { days: i64 },
    NoNewsSinceInterview { at: DateTime<Utc>, days: i64 },
}

impl Job {
    /// The events, latest first; of events at the same moment, the one
    /// logged last first.
    pub fn timeline(&self) -> Vec<&JobEvent> {
        let mut events: Vec<&JobEvent> = self.events.iter().rev().collect();
        events.sort_by_key(|event| std::cmp::Reverse(event.at));
        events
    }

    /// The latest step that has happened. Of steps at the same moment, the
    /// one logged last.
    fn latest_step(&self, now: DateTime<Utc>) -> Option<&JobEvent> {
        self.events
            .iter()
            .filter(|event| event.kind.is_step() && event.at <= now)
            .max_by_key(|event| event.at)
    }

    /// The soonest interview still to come, unless the application closed.
    pub fn next_interview(&self, now: DateTime<Utc>) -> Option<&JobEvent> {
        if self.stage_so_far(now).is_closed() {
            return None;
        }
        self.events
            .iter()
            .filter(|event| {
                matches!(event.kind, JobEventKind::Interview { .. }) && event.at > now
            })
            .min_by_key(|event| event.at)
    }

    /// An interview to come in an application that has since closed, so it
    /// won't happen.
    pub fn is_called_off(&self, event: &JobEvent, now: DateTime<Utc>) -> bool {
        matches!(event.kind, JobEventKind::Interview { .. })
            && event.at > now
            && self.stage_so_far(now).is_closed()
    }

    /// Where the application stands: what has happened, or the next
    /// interview when one is coming. An outcome, like a rejection, outranks
    /// an interview still on the calendar.
    pub fn stage(&self, now: DateTime<Utc>) -> Stage {
        let so_far = self.stage_so_far(now);
        if so_far.is_closed() || so_far == Stage::Offer {
            return so_far;
        }
        match self.next_interview(now).map(|event| (event.at, &event.kind)) {
            Some((at, JobEventKind::Interview { kind, .. })) => Stage::Interview { at, kind: *kind },
            _ => so_far,
        }
    }

    /// The stage from the steps that have happened, ignoring any to come.
    fn stage_so_far(&self, now: DateTime<Utc>) -> Stage {
        match self.latest_step(now).map(|event| (event.at, &event.kind)) {
            None | Some((_, JobEventKind::Note)) => Stage::NotApplied,
            Some((_, JobEventKind::Applied { .. })) => Stage::Applied,
            Some((_, JobEventKind::RecruiterReplied)) => Stage::Replied,
            Some((at, JobEventKind::Interview { kind, .. })) => Stage::Interview { at, kind: *kind },
            Some((_, JobEventKind::Offer)) => Stage::Offer,
            Some((_, JobEventKind::Rejected)) => Stage::Rejected,
            Some((_, JobEventKind::Withdrawn)) => Stage::Withdrawn,
            Some((_, JobEventKind::NoResponse)) => Stage::NoResponse,
        }
    }

    pub fn nudge(&self, now: DateTime<Utc>) -> Option<Nudge> {
        if let Some(interview) = self.next_interview(now) {
            return Some(Nudge::InterviewSoon { at: interview.at });
        }
        let latest = self.latest_step(now)?;
        let days = (now - latest.at).num_days();
        match latest.kind {
            JobEventKind::Interview { .. } if days >= NO_NEWS_AFTER_INTERVIEW_DAYS => {
                Some(Nudge::NoNewsSinceInterview {
                    at: latest.at,
                    days,
                })
            }
            JobEventKind::Applied { .. } | JobEventKind::RecruiterReplied
                if days >= NO_REPLY_DAYS =>
            {
                Some(Nudge::NoReply { days })
            }
            _ => None,
        }
    }

    /// When something last happened, for sorting: the latest past event, or
    /// when the job was added.
    pub fn last_activity(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        self.events
            .iter()
            .map(|event| event.at.min(now))
            .chain([self.created_at])
            .max()
            .unwrap_or(self.created_at)
    }

    /// The résumé sent with the latest application.
    pub fn sent_resume(&self) -> Option<&TailoredResume> {
        self.events
            .iter()
            .filter_map(|event| match event.kind {
                JobEventKind::Applied {
                    resume: Some(resume),
                } => Some((event.at, resume)),
                _ => None,
            })
            .max_by_key(|(at, _)| *at)
            .and_then(|(_, resume)| self.resume(resume))
    }

    /// The résumé sent, when it's no longer the current one.
    pub fn sent_older_resume(&self) -> Option<&TailoredResume> {
        let sent = self.sent_resume()?;
        let current = self.current_resume()?;
        (sent.id != current.id).then_some(sent)
    }
}

impl Library {
    /// Adds the event, or replaces the one with its id.
    pub fn save_event(&mut self, job: JobId, event: JobEvent) {
        let Some(job) = self.job_mut(job) else {
            return;
        };
        match job.events.iter_mut().find(|each| each.id == event.id) {
            Some(existing) => *existing = event,
            None => job.events.push(event),
        }
    }

    /// Removes the event and returns it, to restore on undo.
    pub fn delete_event(&mut self, job: JobId, event: EventId) -> Option<JobEvent> {
        let job = self.job_mut(job)?;
        let index = job.events.iter().position(|each| each.id == event)?;
        Some(job.events.remove(index))
    }
}

/// An event kind without its details, to pick from before filling them in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Applied,
    RecruiterReplied,
    Interview,
    Offer,
    Rejected,
    Withdrawn,
    NoResponse,
    Note,
}

impl StepKind {
    pub const ALL: [Self; 8] = [
        Self::Applied,
        Self::RecruiterReplied,
        Self::Interview,
        Self::Offer,
        Self::Rejected,
        Self::Withdrawn,
        Self::NoResponse,
        Self::Note,
    ];

    pub fn of(kind: &JobEventKind) -> Self {
        match kind {
            JobEventKind::Applied { .. } => Self::Applied,
            JobEventKind::RecruiterReplied => Self::RecruiterReplied,
            JobEventKind::Interview { .. } => Self::Interview,
            JobEventKind::Offer => Self::Offer,
            JobEventKind::Rejected => Self::Rejected,
            JobEventKind::Withdrawn => Self::Withdrawn,
            JobEventKind::NoResponse => Self::NoResponse,
            JobEventKind::Note => Self::Note,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn job_with(events: Vec<(i64, JobEventKind)>) -> (Job, DateTime<Utc>) {
        let now = Utc::now();
        let job = Job {
            id: JobId::new(),
            title: String::new(),
            company: String::new(),
            description: String::new(),
            url: None,
            created_at: now - Duration::days(60),
            overrides: Vec::new(),
            resumes: Vec::new(),
            cover_letter: None,
            events: events
                .into_iter()
                .map(|(days_ago, kind)| JobEvent::new(now - Duration::days(days_ago), kind))
                .collect(),
        };
        (job, now)
    }

    #[test]
    fn the_latest_step_sets_the_stage_and_notes_dont() {
        let (job, _) = job_with(vec![
            (10, JobEventKind::Applied { resume: None }),
            (5, JobEventKind::Rejected),
            (1, JobEventKind::Note),
        ]);
        let now = Utc::now();
        assert_eq!(job.stage(now), Stage::Rejected);
        assert!(job.stage(now).is_closed());
    }

    #[test]
    fn a_quiet_application_gets_a_nudge() {
        let (job, now) = job_with(vec![(20, JobEventKind::Applied { resume: None })]);
        assert_eq!(job.nudge(now), Some(Nudge::NoReply { days: 20 }));
        let (job, now) = job_with(vec![(3, JobEventKind::Applied { resume: None })]);
        assert_eq!(job.nudge(now), None);
    }

    #[test]
    fn an_interview_to_come_is_the_nudge() {
        let interview = JobEventKind::Interview {
            kind: InterviewKind::Phone,
            with: String::new(),
        };
        let (job, now) = job_with(vec![
            (30, JobEventKind::Applied { resume: None }),
            (-3, interview),
        ]);
        assert!(matches!(job.nudge(now), Some(Nudge::InterviewSoon { .. })));
        assert!(job.stage(now).is_interviewing());
    }

    fn interview() -> JobEventKind {
        JobEventKind::Interview {
            kind: InterviewKind::Phone,
            with: String::new(),
        }
    }

    #[test]
    fn a_rejection_outranks_an_interview_still_to_come() {
        let (job, now) = job_with(vec![
            (10, JobEventKind::Applied { resume: None }),
            (-3, interview()),
            (0, JobEventKind::Rejected),
        ]);
        assert_eq!(job.stage(now), Stage::Rejected);
        assert_eq!(job.nudge(now), None);
        let called_off = job.events.iter().any(|event| job.is_called_off(event, now));
        assert!(called_off);
    }

    #[test]
    fn the_soonest_interview_is_the_next_one() {
        let (job, now) = job_with(vec![(-8, interview()), (-3, interview())]);
        let Some(Nudge::InterviewSoon { at }) = job.nudge(now) else {
            panic!("no interview nudge");
        };
        assert_eq!((at - now).num_days(), 3);
    }

    #[test]
    fn of_two_steps_at_the_same_moment_the_one_logged_last_counts() {
        let (mut job, now) = job_with(vec![(1, JobEventKind::NoResponse), (1, JobEventKind::Offer)]);
        let at = job.events[0].at;
        job.events[1].at = at;
        assert_eq!(job.stage(now), Stage::Offer);
    }
}
