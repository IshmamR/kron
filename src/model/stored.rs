// How the model's enums are stored: each variant's text in its column, as the
// schema's CHECKs list them. One table both ways, so a write and a read can't
// disagree.

use super::{EntryKind, FactKind, Inclusion, InterviewKind, ReviewReason, SectionKind};

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
