use rusqlite::Connection;
use uuid::Uuid;

use super::JobId;
use crate::appearance::{Appearance, Motion, Palette, Palettes};
use crate::database::schema::{self, device_settings, settings};
use crate::database::{Error, Write};

/// The settings every device shares, from `settings`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub appearance: Appearance,
    pub motion: Motion,
    pub palettes: Palettes,
}

impl Settings {
    pub fn read(connection: &Connection) -> Result<Self, Error> {
        Ok(connection.query_row(
            "SELECT appearance, motion, light_palette, dark_palette FROM settings",
            [],
            |row| {
                Ok(Self {
                    appearance: row.get(0)?,
                    motion: row.get(1)?,
                    palettes: Palettes {
                        light: row.get(2)?,
                        dark: row.get(3)?,
                    },
                })
            },
        )?)
    }
}

/// This device's own, from `device_settings` in device.sqlite: never synced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceSettings {
    /// As the window was last left; `None`: never, so it opens at the default.
    pub window: Option<SavedWindow>,
    pub sidebar_collapsed: bool,
    pub last_page: LastPage,
}

impl DeviceSettings {
    pub fn read(device: &Connection) -> Result<Self, Error> {
        type Row = (
            Option<WindowState>,
            Option<i32>,
            Option<i32>,
            Option<u32>,
            Option<u32>,
            bool,
            String,
            Option<Uuid>,
        );
        let (state, x, y, width, height, sidebar_collapsed, last_page, last_job): Row = device
            .query_row(
                "SELECT window_state, window_x, window_y, window_width, window_height,
                        sidebar_collapsed, last_page, last_job
                 FROM device_settings",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )?;
        let window = match (state, x, y, width, height) {
            (Some(state), Some(x), Some(y), Some(width), Some(height)) => Some(SavedWindow {
                state,
                x,
                y,
                width,
                height,
            }),
            _ => None,
        };
        let last_page = match (last_page.as_str(), last_job) {
            ("job", Some(job)) => LastPage::Job(JobId(job)),
            ("facts", _) => LastPage::Facts,
            _ => LastPage::Resumes,
        };
        Ok(Self {
            window,
            sidebar_collapsed,
            last_page,
        })
    }
}

/// Where the window was and how, in logical pixels. For a maximized or full
/// screen window, the bounds it restores to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavedWindow {
    pub state: WindowState,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowState {
    Windowed,
    Maximized,
    Fullscreen,
}

/// The page to open on start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LastPage {
    #[default]
    Resumes,
    Job(JobId),
    Facts,
}

pub fn set_appearance(write: &mut Write, appearance: Appearance) -> Result<(), Error> {
    write.set(settings::APPEARANCE, 1, &appearance.stored())
}

pub fn set_motion(write: &mut Write, motion: Motion) -> Result<(), Error> {
    write.set(settings::MOTION, 1, &motion.stored())
}

pub fn set_palette(write: &mut Write, palette: Palette) -> Result<(), Error> {
    let column = if palette.is_dark() {
        settings::DARK_PALETTE
    } else {
        settings::LIGHT_PALETTE
    };
    write.set(column, 1, &palette.stored())
}

/// The made-up name sent to models in place of the user's.
pub fn stand_in_name(connection: &Connection) -> Result<String, Error> {
    Ok(connection.query_row("SELECT stand_in_name FROM settings", [], |row| row.get(0))?)
}

/// Why a stand-in name wasn't saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Empty,
    /// Already in the user's facts, so putting their name back where a model
    /// wrote it would also change that.
    InFacts,
}

/// Saves `name` as the stand-in, or says why not.
pub fn set_stand_in_name(write: &mut Write, name: &str) -> Result<Option<Refused>, Error> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(Some(Refused::Empty));
    }
    let in_facts: bool = write
        .prepare(
            "SELECT EXISTS (SELECT 1 FROM facts
                            WHERE deleted_at IS NULL AND instr(lower(text), lower(?1)) > 0)",
        )?
        .query_row([name], |row| row.get(0))?;
    if in_facts {
        return Ok(Some(Refused::InFacts));
    }
    write.set(settings::STAND_IN_NAME, 1, &name)?;
    Ok(None)
}

pub fn set_sidebar_collapsed(write: &mut Write, collapsed: bool) -> Result<(), Error> {
    write.set(device_settings::SIDEBAR_COLLAPSED, 1, &collapsed)
}

pub fn set_last_page(write: &mut Write, page: LastPage) -> Result<(), Error> {
    let (name, job) = match page {
        LastPage::Resumes => ("resumes", None),
        LastPage::Job(job) => ("job", Some(job.0)),
        LastPage::Facts => ("facts", None),
    };
    write.update(
        schema::DeviceSettings,
        1,
        &[
            device_settings::LAST_PAGE.value(&name),
            device_settings::LAST_JOB.value(&job),
        ],
    )
}

pub fn save_window(write: &mut Write, window: SavedWindow) -> Result<(), Error> {
    write.update(
        schema::DeviceSettings,
        1,
        &[
            device_settings::WINDOW_STATE.value(&Some(window.state.stored())),
            device_settings::WINDOW_X.value(&Some(window.x)),
            device_settings::WINDOW_Y.value(&Some(window.y)),
            device_settings::WINDOW_WIDTH.value(&Some(window.width)),
            device_settings::WINDOW_HEIGHT.value(&Some(window.height)),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Writer;
    use crate::model::commands;

    #[test]
    fn settings_start_at_their_defaults() -> Result<(), Error> {
        let writer = Writer::open_in_memory()?;
        assert_eq!(Settings::read(writer.kron())?, Settings::default());
        assert_eq!(
            DeviceSettings::read(writer.device())?,
            DeviceSettings::default()
        );
        Ok(())
    }

    #[test]
    fn settings_read_back_as_set() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let job = JobId::new();
        let window = SavedWindow {
            state: WindowState::Maximized,
            x: -20,
            y: 40,
            width: 1280,
            height: 800,
        };
        writer.execute(|write| {
            set_appearance(write, Appearance::Dark)?;
            set_motion(write, Motion::Reduced)?;
            set_sidebar_collapsed(write, true)?;
            set_last_page(write, LastPage::Job(job))?;
            save_window(write, window)
        })?;
        assert_eq!(
            Settings::read(writer.kron())?,
            Settings {
                appearance: Appearance::Dark,
                motion: Motion::Reduced,
                palettes: Palettes::default(),
            }
        );
        assert_eq!(
            DeviceSettings::read(writer.device())?,
            DeviceSettings {
                window: Some(window),
                sidebar_collapsed: true,
                last_page: LastPage::Job(job),
            }
        );

        // Leaving the job page forgets the job, as the schema's CHECK asks.
        writer.execute(|write| set_last_page(write, LastPage::Facts))?;
        assert_eq!(
            DeviceSettings::read(writer.device())?.last_page,
            LastPage::Facts
        );
        Ok(())
    }

    #[test]
    fn palette_choices_are_kept_separately_from_appearance() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        writer.execute(|write| {
            set_palette(write, Palette::Glacier)?;
            set_palette(write, Palette::Moss)
        })?;
        let stored = Settings::read(writer.kron())?;
        assert_eq!(stored.appearance, Appearance::System);
        assert_eq!(
            stored.palettes,
            Palettes {
                light: Palette::Glacier,
                dark: Palette::Moss
            }
        );
        writer.execute(|write| set_appearance(write, Appearance::Dark))?;
        assert_eq!(Settings::read(writer.kron())?.palettes, stored.palettes);
        Ok(())
    }

    /// The schema lists the palettes it accepts; one missing there can't be saved.
    #[test]
    fn every_palette_can_be_saved() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        for palette in Palette::ALL {
            writer.execute(|write| set_palette(write, palette))?;
            assert_eq!(
                Settings::read(writer.kron())?
                    .palettes
                    .for_mode(palette.is_dark()),
                palette
            );
        }
        Ok(())
    }

    #[test]
    fn a_stand_in_name_in_the_facts_or_empty_is_refused() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        assert_eq!(stand_in_name(writer.kron())?, "Jordan Ellis");
        writer.execute(|write| commands::add_note(write, "Mentored Sam Okafor, a new hire."))?;
        for (name, refused) in [
            ("  ", Some(Refused::Empty)),
            ("sam okafor", Some(Refused::InFacts)),
            (" Riley Novak ", None),
        ] {
            let (saved, _) = writer.execute(|write| set_stand_in_name(write, name))?;
            assert_eq!(saved, refused, "{name:?}");
        }
        assert_eq!(stand_in_name(writer.kron())?, "Riley Novak");
        Ok(())
    }
}
