// How the model's enums are stored: each variant's text in its column, as the
// schema's CHECKs list them. One table both ways, so a write and a read can't
// disagree.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};

use super::settings::WindowState;
use super::{EntryKind, FactKind, Inclusion, InterviewKind, ReviewReason, SectionKind};
use crate::appearance::{Appearance, Motion};

macro_rules! stored {
    ($type:ty { $($variant:ident => $text:literal),* $(,)? }) => {
        impl $type {
            pub fn stored(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)*
                }
            }

            pub fn from_stored(text: &str) -> Option<Self> {
                match text {
                    $($text => Some(Self::$variant),)*
                    _ => None,
                }
            }
        }

        /// Fails on text the column's CHECK doesn't allow.
        impl FromSql for $type {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                let text = value.as_str()?;
                Self::from_stored(text).ok_or_else(|| {
                    FromSqlError::Other(format!("{}: {text:?}", stringify!($type)).into())
                })
            }
        }
    };
}

stored!(FactKind {
    Name => "name",
    Bullet => "bullet",
    Title => "title",
    Organization => "organization",
    Date => "date",
    Skill => "skill",
    Contact => "contact",
    Summary => "summary",
    Location => "location",
    SkillGroup => "skill_group",
});

stored!(EntryKind {
    Experience => "experience",
    Education => "education",
    Project => "project",
    Certification => "certification",
    Skills => "skills",
    Other => "custom",
});

stored!(SectionKind {
    Header => "header",
    Summary => "summary",
    Experience => "experience",
    Education => "education",
    Projects => "projects",
    Skills => "skills",
    Other => "custom",
});

stored!(InterviewKind {
    Phone => "phone",
    Technical => "technical",
    Onsite => "onsite",
    Other => "other",
});

stored!(ReviewReason {
    Cleaned => "cleaned",
    Unsure => "unsure",
    Structure => "structure",
    Duplicate => "duplicate",
});

stored!(Inclusion {
    Include => "include",
    Exclude => "exclude",
});

stored!(Appearance {
    System => "system",
    Light => "light",
    Dark => "dark",
});

stored!(Motion {
    System => "system",
    Reduced => "reduced",
    Full => "full",
});

stored!(WindowState {
    Windowed => "windowed",
    Maximized => "maximized",
    Fullscreen => "fullscreen",
});
