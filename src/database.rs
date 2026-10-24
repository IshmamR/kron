pub mod schema;
mod write;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;

use futures::StreamExt;
use futures::channel::{mpsc as async_mpsc, oneshot};
use gpui_kit::{App, AppContext, Context, Entity, EventEmitter, Subscription, Task};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use uuid::Uuid;

use schema::{DEVICE_MIGRATIONS, KRON_MIGRATIONS, Key, Table};

pub use write::{Write, Writer};

/// DB names
const KRON_DB: &str = "kron.sqlite";
const DEVICE_DB: &str = "device.sqlite";
/// Imported files and templates, by hash, in the data directory.
const FILES_DIRECTORY: &str = "files";
/// The folder in the platform's data directory. Debug builds use their own,
/// so development never touches real data and resets by deleting it.
#[cfg(not(debug_assertions))]
const DATA_DIRECTORY: &str = "kron";
#[cfg(debug_assertions)]
const DATA_DIRECTORY: &str = "kron-dev";

/// Opens the databases in the data directory, migrating them, and starts the
/// writer thread.
pub fn open(cx: &mut App) -> Result<Entity<Database>, Error> {
    let opened = Opened::open(&data_directory()?)?;
    Ok(cx.new(|cx| Database::new(opened, cx)))
}

/// `open` in a directory of the test's own.
#[cfg(test)]
pub fn open_in(directory: &TemporaryDirectory, cx: &mut App) -> Result<Entity<Database>, Error> {
    let opened = Opened::open(&directory.0)?;
    Ok(cx.new(|cx| Database::new(opened, cx)))
}

/// The connections, and the only state shared between views. It knows
/// nothing about résumés: the domain modules own their tables and write them
/// through commands, and each view runs its own queries on `connection`.
pub struct Database {
    /// Read-only, on the main thread: queries take well under a ms.
    reader: Connection,
    /// device.sqlite, read-only too.
    device_reader: Connection,
    device_id: Uuid,
    writer: WriterThread,
    _changes: Task<()>,
    _quit: Subscription,
}

impl EventEmitter<Changed> for Database {}

impl Database {
    fn new(opened: Opened, cx: &mut Context<Self>) -> Self {
        let (changes, mut changed) = async_mpsc::unbounded();
        let writer = WriterThread::spawn(opened.writer, changes);
        Self {
            reader: opened.reader,
            device_reader: opened.device_reader,
            device_id: opened.device_id,
            writer,
            _changes: cx.spawn(async move |this, cx| {
                while let Some(event) = changed.next().await {
                    if this.update(cx, |_, cx| cx.emit(event)).is_err() {
                        break;
                    }
                }
            }),
            // Finishes the queued writes before the process exits.
            _quit: cx.on_app_quit(|this, _| {
                this.writer.close();
                async {}
            }),
        }
    }

    /// For queries. Read-only: changes go through `write`.
    pub fn connection(&self) -> &Connection {
        &self.reader
    }

    /// For queries of this device's tables, like `device_settings`.
    pub fn device_connection(&self) -> &Connection {
        &self.device_reader
    }

    pub fn device_id(&self) -> Uuid {
        self.device_id
    }

    /// Queues `command` on the writer thread, after any queued before it,
    /// and resolves to what it returned once committed. Dropping the task
    /// only stops waiting: the command still runs, and views still hear of
    /// what it changed.
    pub fn write<R: Send + 'static>(
        &self,
        cx: &App,
        command: impl FnOnce(&mut Write) -> Result<R, Error> + Send + 'static,
    ) -> Task<Result<R, Error>> {
        let outcome = self.writer.submit(command);
        cx.background_spawn(async move { outcome.await.unwrap_or(Err(Error::WriterStopped)) })
    }

    /// `write` for a command whose result nobody needs: views hear of its
    /// changes through `Changed`, and the writer logs a failure.
    pub fn queue(&self, command: impl FnOnce(&mut Write) -> Result<(), Error> + Send + 'static) {
        drop(self.writer.submit(command));
    }

    /// Waits for the commands queued so far to commit. Their `Changed` events
    /// are sent by then, for the test to deliver.
    #[cfg(test)]
    pub fn flush(&self) {
        let flushed = futures::executor::block_on(self.writer.submit(|_| Ok(())));
        assert!(matches!(flushed, Ok(Ok(()))), "the writer has stopped");
    }
}

/// What a committed command changed, so each view can tell whether it shows
/// any of it and query again.
#[derive(Clone, Debug)]
pub struct Changed {
    pub changes: Vec<Change>,
}

impl Changed {
    pub fn touches<T: Table>(&self, _table: T) -> bool {
        self.changes.iter().any(|change| change.table == T::NAME)
    }

    pub fn touches_row<T: Table>(&self, _table: T, key: impl Key<T::Key>) -> bool {
        // Fails only for a value no key column holds, so no row of it changed.
        write::row_key(&key.values()).is_ok_and(|key| {
            self.changes
                .iter()
                .any(|change| change.table == T::NAME && *change.key == *key)
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub table: &'static str,
    /// The row's primary key values joined, as in `outbox.row_key`.
    pub key: Box<[u8]>,
    /// `None`: the row was added.
    pub field: Option<&'static str>,
}

#[derive(Debug)]
pub enum Error {
    NoDataDirectory,
    Directory(PathBuf, std::io::Error),
    File(PathBuf, std::io::Error),
    Sqlite(rusqlite::Error),
    /// A newer Kron migrated it further than this one knows; opening it
    /// anyway could write what that version doesn't expect.
    NewerSchema {
        database: &'static str,
        version: u32,
    },
    /// A command named a table or column that doesn't exist, gave the wrong
    /// key, or tried to write around `Write`. A bug, never the user's doing.
    Misuse(String),
    RowNotFound {
        table: &'static str,
    },
    /// The writer thread is gone, after the app started quitting or a
    /// command panicked.
    WriterStopped,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::NoDataDirectory => write!(formatter, "this platform has no data directory"),
            Error::Directory(path, error) | Error::File(path, error) => {
                write!(formatter, "{}: {error}", path.display())
            }
            Error::Sqlite(error) => write!(formatter, "{error}"),
            Error::NewerSchema { database, version } => write!(
                formatter,
                "{database} is at schema version {version}, made by a newer Kron"
            ),
            Error::Misuse(message) => write!(formatter, "database misuse: {message}"),
            Error::RowNotFound { table } => write!(formatter, "no such row in {table}"),
            Error::WriterStopped => write!(formatter, "the database writer has stopped"),
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Error::Sqlite(error)
    }
}

/// Both databases opened and migrated, before the entity exists.
struct Opened {
    reader: Connection,
    device_reader: Connection,
    writer: Writer,
    device_id: Uuid,
}

impl Opened {
    fn open(directory: &Path) -> Result<Self, Error> {
        std::fs::create_dir_all(directory)
            .map_err(|error| Error::Directory(directory.to_path_buf(), error))?;
        let kron_path = directory.join(KRON_DB);
        let device_path = directory.join(DEVICE_DB);
        let mut kron = open_connection(&kron_path, OpenFlags::default())?;
        let mut device = open_connection(&device_path, OpenFlags::default())?;
        migrate(&mut kron, KRON_DB, KRON_MIGRATIONS)?;
        migrate(&mut device, DEVICE_DB, DEVICE_MIGRATIONS)?;
        let device_id = device_id(&device)?;
        // Opened after migrating, so the files and their WALs exist, which a
        // read-only connection can't create.
        let read_only = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let reader = open_connection(&kron_path, read_only)?;
        reader.set_prepared_statement_cache_capacity(write::STATEMENT_CACHE);
        Ok(Self {
            reader,
            device_reader: open_connection(&device_path, read_only)?,
            writer: Writer::new(kron, device, Some(directory.join(FILES_DIRECTORY)))?,
            device_id,
        })
    }
}

type Command = Box<dyn FnOnce(&mut Writer) + Send>;

/// Runs commands one at a time, in the order queued, on a thread of its own,
/// so a write never holds up a frame.
struct WriterThread {
    /// `None` once closed.
    queue: Option<mpsc::Sender<Command>>,
    thread: Option<JoinHandle<()>>,
    changes: async_mpsc::UnboundedSender<Changed>,
}

impl WriterThread {
    fn spawn(mut writer: Writer, changes: async_mpsc::UnboundedSender<Changed>) -> Self {
        let (queue, commands) = mpsc::channel::<Command>();
        let thread = std::thread::Builder::new()
            .name("database writer".into())
            .spawn(move || {
                for command in commands {
                    command(&mut writer);
                }
            });
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(error) => {
                eprintln!("couldn't start the database writer: {error}");
                None
            }
        };
        Self {
            queue: thread.is_some().then_some(queue),
            thread,
            changes,
        }
    }

    /// Resolves to the command's outcome, or is cancelled if the writer
    /// stopped before running it.
    fn submit<R: Send + 'static>(
        &self,
        command: impl FnOnce(&mut Write) -> Result<R, Error> + Send + 'static,
    ) -> oneshot::Receiver<Result<R, Error>> {
        let (outcome, receiver) = oneshot::channel();
        let changes = self.changes.clone();
        let queued: Command = Box::new(move |writer| {
            let result = writer.execute(command).map(|(value, changed)| {
                // Fails only once the entity is gone, with no view left to tell.
                if !changed.is_empty()
                    && changes
                        .unbounded_send(Changed { changes: changed })
                        .is_err()
                {
                    eprintln!("database changes after the database closed");
                }
                value
            });
            // Nobody waits when the caller dropped its task. The changes went
            // to the views above, so only a failure needs logging.
            if let Err(Err(error)) = outcome.send(result) {
                eprintln!("a command nobody waited for failed: {error}");
            }
        });
        if let Some(queue) = &self.queue
            && queue.send(queued).is_err()
        {
            eprintln!("the database writer has stopped");
        }
        receiver
    }

    /// Lets the queued commands finish, then stops the thread.
    fn close(&mut self) {
        self.queue.take();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("the database writer panicked");
        }
    }
}

impl Drop for WriterThread {
    fn drop(&mut self) {
        self.close();
    }
}

/// `~/.local/share/kron` on Linux, `~/Library/Application Support/kron` on
/// macOS, `%APPDATA%\kron` on Windows; `kron-dev` in debug builds.
fn data_directory() -> Result<PathBuf, Error> {
    dirs::data_dir()
        .map(|directory| directory.join(DATA_DIRECTORY))
        .ok_or(Error::NoDataDirectory)
}

/// A directory of its own, removed when dropped.
#[cfg(test)]
pub struct TemporaryDirectory(PathBuf);

#[cfg(test)]
impl TemporaryDirectory {
    pub fn new() -> Self {
        Self(std::env::temp_dir().join(format!("kron-test-{}", Uuid::now_v7())))
    }
}

#[cfg(test)]
impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("couldn't remove {}: {error}", self.0.display());
        }
    }
}

fn open_connection(path: &Path, flags: OpenFlags) -> Result<Connection, Error> {
    let connection = Connection::open_with_flags(path, flags)?;
    if !flags.contains(OpenFlags::SQLITE_OPEN_READ_ONLY) {
        // WAL lets the UI read while a write is in progress, and with it
        // NORMAL only risks the last commits on power loss, never corruption.
        let journal_mode: String =
            connection.pragma_update_and_check(None, "journal_mode", "wal", |row| row.get(0))?;
        if journal_mode != "wal" {
            eprintln!(
                "{}: journal mode is {journal_mode}, not wal",
                path.display()
            );
        }
        connection.pragma_update(None, "synchronous", "normal")?;
    }
    Ok(connection)
}

/// Applies the migrations the database hasn't had, each in its own
/// transaction with the version bump, so a crash leaves it at a version.
fn migrate(
    connection: &mut Connection,
    database: &'static str,
    migrations: &[&str],
) -> Result<(), Error> {
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version as usize > migrations.len() {
        return Err(Error::NewerSchema { database, version });
    }
    for (next_version, migration) in (1u32..).zip(migrations).skip(version as usize) {
        let transaction = connection.transaction()?;
        transaction.execute_batch(migration)?;
        transaction.pragma_update(None, "user_version", next_version)?;
        transaction.commit()?;
    }
    // After migrating: a future migration that rebuilds a table needs them
    // off, and the pragma does nothing inside a transaction.
    connection.pragma_update(None, "foreign_keys", true)?;
    Ok(())
}

/// This install's ID, made on first open.
fn device_id(device: &Connection) -> Result<Uuid, Error> {
    let saved: Option<[u8; 16]> = device
        .query_row("SELECT id FROM device", [], |row| row.get(0))
        .optional()?;
    if let Some(bytes) = saved {
        return Ok(Uuid::from_bytes(bytes));
    }
    let id = Uuid::now_v7();
    device.execute(
        "INSERT INTO device (singleton, id, clock) VALUES (1, ?1, 0)",
        [id.as_bytes().as_slice()],
    )?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;

    use super::*;
    use schema::{Documents, Facts, Jobs, documents};

    fn add_note(write: &mut Write, id: Uuid) -> Result<(), Error> {
        let now = write.now();
        write.insert(Documents, id, &[documents::ADDED_AT.value(&now)])
    }

    fn count(connection: &Connection, table: &str) -> rusqlite::Result<i64> {
        connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
    }

    #[test]
    fn migrates_to_the_latest_version_with_the_one_row_tables() -> Result<(), Error> {
        let directory = TemporaryDirectory::new();
        let opened = Opened::open(&directory.0)?;
        let version: u32 = opened
            .reader
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(version as usize, KRON_MIGRATIONS.len());
        let device = opened.writer.device();
        let version: u32 = device.pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(version as usize, DEVICE_MIGRATIONS.len());
        assert_eq!(count(&opened.reader, "settings")?, 1);
        assert_eq!(count(device, "device")?, 1);
        assert_eq!(count(device, "device_settings")?, 1);
        Ok(())
    }

    #[test]
    fn reopening_keeps_the_device_id() -> Result<(), Error> {
        let directory = TemporaryDirectory::new();
        let first = Opened::open(&directory.0)?.device_id;
        let second = Opened::open(&directory.0)?.device_id;
        assert_eq!(first, second);
        Ok(())
    }

    #[test]
    fn writes_use_wal_and_foreign_keys_and_the_reader_cannot_write() -> Result<(), Error> {
        let directory = TemporaryDirectory::new();
        let opened = Opened::open(&directory.0)?;
        let kron = opened.writer.kron();
        let journal_mode: String =
            kron.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        let foreign_keys: bool = kron.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
        assert_eq!(journal_mode, "wal");
        assert!(foreign_keys);
        let written = opened.reader.execute("DELETE FROM settings", []);
        assert!(matches!(
            written,
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ReadOnly
        ));
        Ok(())
    }

    #[test]
    fn a_database_from_an_older_version_migrates_and_keeps_its_rows() -> Result<(), Error> {
        let mut connection = Connection::open_in_memory()?;
        migrate(&mut connection, "kron.sqlite", &KRON_MIGRATIONS[..1])?;
        connection.execute(
            "INSERT INTO documents (id, added_at) VALUES (?1, 0)",
            [Uuid::now_v7()],
        )?;
        migrate(&mut connection, "kron.sqlite", KRON_MIGRATIONS)?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(version as usize, KRON_MIGRATIONS.len());
        assert_eq!(count(&connection, "documents")?, 1);
        let indexes: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'index'
             AND name IN ('facts_by_merged_into', 'line_citations_by_fact', 'lines_by_block')",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(indexes, 3);
        Ok(())
    }

    #[test]
    fn refuses_a_newer_schema() -> Result<(), Error> {
        let mut connection = Connection::open_in_memory()?;
        connection.pragma_update(None, "user_version", 99)?;
        let result = migrate(&mut connection, "kron.sqlite", KRON_MIGRATIONS);
        assert!(matches!(
            result,
            Err(Error::NewerSchema { version: 99, .. })
        ));
        Ok(())
    }

    #[test]
    fn the_reader_sees_a_command_once_it_resolves_and_views_hear_of_it() -> Result<(), Error> {
        let directory = TemporaryDirectory::new();
        let opened = Opened::open(&directory.0)?;
        let (changes, mut changed) = async_mpsc::unbounded();
        let writer = WriterThread::spawn(opened.writer, changes);
        let id = Uuid::now_v7();
        block_on(writer.submit(move |write| add_note(write, id)))
            .unwrap_or(Err(Error::WriterStopped))?;
        assert_eq!(count(&opened.reader, "documents")?, 1);
        let event = block_on(changed.next());
        assert!(event.is_some_and(|event| event.touches_row(Documents, id)));
        Ok(())
    }

    #[test]
    fn commands_run_in_order_and_closing_finishes_the_queue() -> Result<(), Error> {
        let directory = TemporaryDirectory::new();
        let opened = Opened::open(&directory.0)?;
        let (changes, _changed) = async_mpsc::unbounded();
        let mut writer = WriterThread::spawn(opened.writer, changes);
        let ids: Vec<Uuid> = (0..50).map(|_| Uuid::now_v7()).collect();
        // Nobody waits for these: dropping the receiver doesn't cancel them.
        for &id in &ids {
            drop(writer.submit(move |write| add_note(write, id)));
        }
        writer.close();
        let written: Vec<[u8; 16]> = opened
            .reader
            .prepare("SELECT id FROM documents ORDER BY rowid")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let expected: Vec<[u8; 16]> = ids.iter().map(|id| *id.as_bytes()).collect();
        assert_eq!(written, expected);
        Ok(())
    }

    #[test]
    fn a_closed_writer_reports_it() -> Result<(), Error> {
        let directory = TemporaryDirectory::new();
        let opened = Opened::open(&directory.0)?;
        let (changes, _changed) = async_mpsc::unbounded();
        let mut writer = WriterThread::spawn(opened.writer, changes);
        writer.close();
        let outcome = block_on(writer.submit(|_| Ok(())));
        assert!(outcome.is_err());
        Ok(())
    }

    #[test]
    fn changed_tells_tables_and_rows_apart() {
        let id = Uuid::now_v7();
        let changed = Changed {
            changes: vec![Change {
                table: "facts",
                key: id.as_bytes().as_slice().into(),
                field: Some("text"),
            }],
        };
        assert!(changed.touches(Facts));
        assert!(!changed.touches(Jobs));
        assert!(changed.touches_row(Facts, id));
        assert!(!changed.touches_row(Facts, Uuid::now_v7()));
        assert!(!changed.touches_row(Jobs, id));
    }
}
