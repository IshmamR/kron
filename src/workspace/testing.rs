// What the views' tests share: a database on disk with the sample, and
// letting its writer catch up.

use gpui_kit::{Entity, TestAppContext, VisualTestContext};

use crate::database::{self, Database, Error, TemporaryDirectory, Write};
use crate::model::sample;
use crate::typeset;

/// Initializes the app's components, and opens a database of its own with
/// the bundled templates and the sample.
pub fn open_sample(app: &mut TestAppContext) -> (Entity<Database>, TemporaryDirectory) {
    let (database, directory) = open_empty(app);
    database.read_with(app, |database, _| {
        database.queue(write_sample);
        database.flush();
    });
    app.run_until_parked();
    (database, directory)
}

/// Initializes the app's components, and opens an empty database of its own,
/// as a new user has.
pub fn open_empty(app: &mut TestAppContext) -> (Entity<Database>, TemporaryDirectory) {
    let directory = TemporaryDirectory::new();
    let database = app.update(|cx| {
        gpui_kit::init(cx);
        super::init(cx);
        database::open_in(&directory, cx)
    });
    match database {
        Ok(database) => (database, directory),
        Err(error) => panic!("couldn't open the database: {error}"),
    }
}

pub fn write_sample(write: &mut Write) -> Result<(), Error> {
    typeset::install_templates(write)?;
    sample::write_sample(write)
}

/// Lets queued commands commit, delivers what they changed, and draws.
pub fn settle(cx: &mut VisualTestContext, database: &Entity<Database>) {
    database.read_with(cx, |database, _| database.flush());
    cx.run_until_parked();
}

/// Runs a command as a view would, and settles.
pub fn run(
    cx: &mut VisualTestContext,
    database: &Entity<Database>,
    command: impl FnOnce(&mut Write) -> Result<(), Error> + Send + 'static,
) {
    database.read_with(cx, |database, _| database.queue(command));
    settle(cx, database);
}

/// Undoes the last change, as Ctrl+Z does, and settles.
pub fn undo(cx: &mut VisualTestContext, database: &Entity<Database>) {
    // Dropping the task only stops waiting: the undo still runs.
    database.read_with(cx, |database, cx| drop(database.undo(cx)));
    settle(cx, database);
}
