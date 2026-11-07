// Where an application stands: its events, and what follows from them.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, Row};
use uuid::Uuid;

use super::{EventId, JobId, ResumeId};
use crate::database::Error;

/// After this long with no answer to an application, it's worth a follow-up,
/// or closing it.
const NO_REPLY_DAYS: i64 = 14;
/// After an interview, news usually comes sooner.
const NO_NEWS_AFTER_INTERVIEW_DAYS: i64 = 7;

/// Something that happened, or will, in an application.
#[derive(Clone, Debug)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobEventKind {
    /// `resume` is the version sent, which regenerating doesn't change.
    Applied {
        resume: Option<ResumeId>,
    },
    RecruiterReplied,
    Interview {
        kind: InterviewKind,
        with: String,
    },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    Interview {
        at: DateTime<Utc>,
        kind: InterviewKind,
    },
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
    InterviewSoon {
        at: DateTime<Utc>,
    },
    /// Applied or heard back, then nothing for a while.
    NoReply {
        days: i64,
    },
    NoNewsSinceInterview {
        at: DateTime<Utc>,
        days: i64,
    },
}

/// A job, and the events of its application.
#[derive(Clone, Debug)]
pub struct Application {
    pub job: JobId,
    /// Short summarized title, shown as the grid caption.
    pub title: String,
    pub company: String,
    pub created_at: DateTime<Utc>,
    /// In the order they were logged.
    pub events: Vec<JobEvent>,
}

impl Application {
    /// The job's application, unless the job is deleted.
    pub fn read(connection: &Connection, job: JobId) -> Result<Option<Self>, Error> {
        Ok(Self::read_where(connection, Some(job))?.into_iter().next())
    }

    /// Every job's application, in the board's order.
    pub fn read_all(connection: &Connection) -> Result<Vec<Self>, Error> {
        Self::read_where(connection, None)
    }

    /// One job's, or with `None` every job's. Separate statements, so one
    /// job's are found by its index rather than by reading every job's.
    fn read_where(connection: &Connection, job: Option<JobId>) -> Result<Vec<Self>, Error> {
        let (jobs, events) = match job {
            Some(_) => (
                "SELECT id, title, company, created_at FROM jobs
                 WHERE id = ?1 AND deleted_at IS NULL",
                // The steps the model doesn't have yet (wishlisted, tailoring,
                // ready) are left out: without them a job hasn't been applied to.
                "SELECT job_id, id, at, kind, resume_id, interview_kind, interview_with, note
                 FROM events
                 WHERE job_id = ?1 AND deleted_at IS NULL
                   AND kind NOT IN ('wishlisted', 'tailoring', 'ready')
                 ORDER BY id",
            ),
            None => (
                "SELECT id, title, company, created_at FROM jobs
                 WHERE deleted_at IS NULL
                 ORDER BY board_sort_key, id",
                "SELECT job_id, id, at, kind, resume_id, interview_kind, interview_with, note
                 FROM events
                 WHERE deleted_at IS NULL AND kind NOT IN ('wishlisted', 'tailoring', 'ready')
                 ORDER BY id",
            ),
        };
        let parameters = rusqlite::params_from_iter(job.map(|job| job.0));
        let mut applications: Vec<Self> = connection
            .prepare_cached(jobs)?
            .query_map(parameters.clone(), |row| {
                Ok(Self {
                    job: JobId(row.get(0)?),
                    title: row.get(1)?,
                    company: row.get(2)?,
                    created_at: time(row.get(3)?),
                    events: Vec::new(),
                })
            })?
            .collect::<Result<_, _>>()?;
        let events: Vec<(Uuid, JobEvent)> = connection
            .prepare_cached(events)?
            .query_map(parameters, |row| Ok((row.get(0)?, event(row)?)))?
            .collect::<Result<_, _>>()?;
        let positions: HashMap<Uuid, usize> = applications
            .iter()
            .enumerate()
            .map(|(position, application)| (application.job.0, position))
            .collect();
        for (job, event) in events {
            if let Some(application) = positions
                .get(&job)
                .and_then(|&position| applications.get_mut(position))
            {
                application.events.push(event);
            }
        }
        Ok(applications)
    }

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
            .filter(|event| matches!(event.kind, JobEventKind::Interview { .. }) && event.at > now)
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
        match self
            .next_interview(now)
            .map(|event| (event.at, &event.kind))
        {
            Some((at, JobEventKind::Interview { kind, .. })) => {
                Stage::Interview { at, kind: *kind }
            }
            _ => so_far,
        }
    }

    /// The stage from the steps that have happened, ignoring any to come.
    fn stage_so_far(&self, now: DateTime<Utc>) -> Stage {
        match self.latest_step(now).map(|event| (event.at, &event.kind)) {
            None | Some((_, JobEventKind::Note)) => Stage::NotApplied,
            Some((_, JobEventKind::Applied { .. })) => Stage::Applied,
            Some((_, JobEventKind::RecruiterReplied)) => Stage::Replied,
            Some((at, JobEventKind::Interview { kind, .. })) => {
                Stage::Interview { at, kind: *kind }
            }
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

    /// The résumé sent with the latest application, if one was recorded.
    pub fn sent_resume(&self) -> Option<ResumeId> {
        self.events
            .iter()
            .filter_map(|event| match event.kind {
                JobEventKind::Applied {
                    resume: Some(resume),
                } => Some((event.at, resume)),
                _ => None,
            })
            .max_by_key(|(at, _)| *at)
            .map(|(_, resume)| resume)
    }
}

/// An event from its row: job_id, id, at, kind, resume_id, interview_kind,
/// interview_with, note.
fn event(row: &Row) -> rusqlite::Result<JobEvent> {
    let kind: String = row.get(3)?;
    let kind = match kind.as_str() {
        "applied" => JobEventKind::Applied {
            resume: row.get::<_, Option<Uuid>>(4)?.map(ResumeId),
        },
        "recruiter_replied" => JobEventKind::RecruiterReplied,
        "interview" => JobEventKind::Interview {
            kind: row.get(5)?,
            with: row.get(6)?,
        },
        "offer" => JobEventKind::Offer,
        "rejected" => JobEventKind::Rejected,
        "withdrawn" => JobEventKind::Withdrawn,
        "no_response" => JobEventKind::NoResponse,
        "note" => JobEventKind::Note,
        other => {
            return Err(rusqlite::Error::InvalidColumnType(
                3,
                format!("events.kind: {other}"),
                rusqlite::types::Type::Text,
            ));
        }
    };
    Ok(JobEvent {
        id: EventId(row.get(1)?),
        at: time(row.get(2)?),
        kind,
        note: row.get(7)?,
    })
}

fn time(milliseconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(milliseconds).unwrap_or_default()
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

    fn application_with(events: Vec<(i64, JobEventKind)>) -> (Application, DateTime<Utc>) {
        let now = Utc::now();
        let application = Application {
            job: JobId::new(),
            title: String::new(),
            company: String::new(),
            created_at: now - Duration::days(60),
            events: events
                .into_iter()
                .map(|(days_ago, kind)| JobEvent::new(now - Duration::days(days_ago), kind))
                .collect(),
        };
        (application, now)
    }

    #[test]
    fn the_latest_step_sets_the_stage_and_notes_dont() {
        let (application, _) = application_with(vec![
            (10, JobEventKind::Applied { resume: None }),
            (5, JobEventKind::Rejected),
            (1, JobEventKind::Note),
        ]);
        let now = Utc::now();
        assert_eq!(application.stage(now), Stage::Rejected);
        assert!(application.stage(now).is_closed());
    }

    #[test]
    fn a_quiet_application_gets_a_nudge() {
        let (application, now) =
            application_with(vec![(20, JobEventKind::Applied { resume: None })]);
        assert_eq!(application.nudge(now), Some(Nudge::NoReply { days: 20 }));
        let (application, now) =
            application_with(vec![(3, JobEventKind::Applied { resume: None })]);
        assert_eq!(application.nudge(now), None);
    }

    #[test]
    fn an_interview_to_come_is_the_nudge() {
        let (application, now) = application_with(vec![
            (30, JobEventKind::Applied { resume: None }),
            (-3, interview()),
        ]);
        assert!(matches!(
            application.nudge(now),
            Some(Nudge::InterviewSoon { .. })
        ));
        assert!(application.stage(now).is_interviewing());
    }

    fn interview() -> JobEventKind {
        JobEventKind::Interview {
            kind: InterviewKind::Phone,
            with: String::new(),
        }
    }

    #[test]
    fn a_rejection_outranks_an_interview_still_to_come() {
        let (application, now) = application_with(vec![
            (10, JobEventKind::Applied { resume: None }),
            (-3, interview()),
            (0, JobEventKind::Rejected),
        ]);
        assert_eq!(application.stage(now), Stage::Rejected);
        assert_eq!(application.nudge(now), None);
        let called_off = application
            .events
            .iter()
            .any(|event| application.is_called_off(event, now));
        assert!(called_off);
    }

    #[test]
    fn the_soonest_interview_is_the_next_one() {
        let (application, now) = application_with(vec![(-8, interview()), (-3, interview())]);
        let Some(Nudge::InterviewSoon { at }) = application.nudge(now) else {
            panic!("no interview nudge");
        };
        assert_eq!((at - now).num_days(), 3);
    }

    #[test]
    fn of_two_steps_at_the_same_moment_the_one_logged_last_counts() {
        let (mut application, now) = application_with(vec![
            (1, JobEventKind::NoResponse),
            (1, JobEventKind::Offer),
        ]);
        let at = application.events[0].at;
        application.events[1].at = at;
        assert_eq!(application.stage(now), Stage::Offer);
    }

    #[test]
    fn the_sample_applications_read_back_in_the_boards_order() -> Result<(), Error> {
        let writer = super::super::sample::written()?;
        let now = Utc::now();
        let stage = |stage: Stage| match stage {
            Stage::NotApplied => "not applied",
            Stage::Applied => "applied",
            Stage::Replied => "replied",
            Stage::Interview { .. } => "interview",
            Stage::Offer => "offer",
            Stage::Rejected => "rejected",
            Stage::Withdrawn => "withdrawn",
            Stage::NoResponse => "no response",
        };
        let read: Vec<(String, usize, &str, bool)> = Application::read_all(writer.kron())?
            .iter()
            .map(|each| {
                (
                    each.company.clone(),
                    each.events.len(),
                    stage(each.stage(now)),
                    each.sent_resume().is_some(),
                )
            })
            .collect();
        // Company, events (`wishlisted` isn't one), stage, and whether a
        // résumé was sent.
        let expected = [
            ("Cinder Labs", 1, "not applied", false),
            ("Ledgerly", 3, "interview", true),
            ("Atlas Freight", 1, "applied", true),
            ("Tidewater Systems", 1, "applied", true),
            ("Quillpay", 3, "interview", true),
            ("Northstar Metrics", 5, "rejected", true),
            ("Brightwave", 5, "offer", true),
            ("Kestrel Labs", 3, "withdrawn", true),
            ("Halcyon Cloud", 0, "not applied", false),
        ];
        assert_eq!(
            read,
            expected
                .map(|(company, events, stage, sent)| (company.to_string(), events, stage, sent))
                .to_vec()
        );
        for each in Application::read_all(writer.kron())? {
            let one = Application::read(writer.kron(), each.job)?;
            assert_eq!(one.map(|one| one.events.len()), Some(each.events.len()));
        }
        assert!(Application::read(writer.kron(), JobId::new())?.is_none());
        Ok(())
    }
}
