// Writes the sample data in `assets/sample/sample.sql`, for building the UI
// before import and generation exist. Development only: release builds start
// empty.

use crate::database::{self, Write};

const SAMPLE: &str = include_str!("../../assets/sample/sample.sql");
/// What the sample's imported documents are saved as: their text, since the
/// sample has no PDFs.
const DOCUMENT_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// Writes the sample unless the database has documents or jobs already, so a
/// first run has something on every screen.
pub fn write_if_empty(write: &mut Write) -> Result<(), database::Error> {
    let has_data: bool = write
        .prepare("SELECT EXISTS (SELECT 1 FROM documents) OR EXISTS (SELECT 1 FROM jobs)")?
        .query_row([], |row| row.get(0))?;
    if has_data {
        Ok(())
    } else {
        write_sample(write)
    }
}

/// Writes the sample into the database. Its templates must be installed
/// (`typeset::install_templates`).
pub fn write_sample(write: &mut Write) -> Result<(), database::Error> {
    write.seed(SAMPLE)?;
    // SQL can't save files, so the imported documents' are saved from their
    // text.
    let documents: Vec<([u8; 32], String)> = write
        .prepare(
            "SELECT documents.file_hash, document_texts.text
             FROM documents JOIN document_texts ON document_texts.document_id = documents.id
             WHERE documents.file_hash IS NOT NULL",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (hash, text) in documents {
        if write.add_file(text.as_bytes(), DOCUMENT_MEDIA_TYPE)? != hash {
            return Err(database::Error::Misuse(
                "a sample document's file_hash isn't the hash of its text".into(),
            ));
        }
    }
    Ok(())
}

/// An in-memory database with the bundled templates and the sample.
#[cfg(test)]
pub fn written() -> Result<database::Writer, database::Error> {
    let mut writer = database::Writer::open_in_memory()?;
    writer.execute(|write| {
        crate::typeset::install_templates(write)?;
        write_sample(write)
    })?;
    Ok(writer)
}

/// The sample fact whose text starts with `start`.
#[cfg(test)]
pub fn fact(
    connection: &rusqlite::Connection,
    start: &str,
) -> Result<super::FactId, database::Error> {
    Ok(super::FactId(connection.query_row(
        "SELECT id FROM facts WHERE text LIKE ?1 || '%' AND merged_into IS NULL
         ORDER BY id LIMIT 1",
        [start],
        |row| row.get(0),
    )?))
}

/// The sample job at `company`.
#[cfg(test)]
pub fn job(
    connection: &rusqlite::Connection,
    company: &str,
) -> Result<super::JobId, database::Error> {
    Ok(super::JobId(connection.query_row(
        "SELECT id FROM jobs WHERE company = ?1",
        [company],
        |row| row.get(0),
    )?))
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::database::{Error, Writer};
    use crate::typeset;

    fn count(connection: &Connection, table: &str) -> Result<i64, Error> {
        Ok(
            connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })?,
        )
    }

    #[test]
    fn the_sample_fits_the_schema_once() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        for _ in 0..2 {
            writer.execute(|write| {
                typeset::install_templates(write)?;
                write_if_empty(write)
            })?;
        }
        let kron = writer.kron();
        assert_eq!(count(kron, "documents")?, 4);
        assert_eq!(
            count(
                kron,
                "files JOIN documents ON documents.file_hash = files.hash"
            )?,
            2,
            "one for each imported document"
        );
        assert_eq!(count(kron, "facts")?, 83);
        assert_eq!(count(kron, "jobs")?, 9);
        assert_eq!(count(kron, "generations WHERE kind = 'resume'")?, 9);
        assert_eq!(
            count(kron, "template_versions")?,
            typeset::TEMPLATES.len() as i64
        );
        // Every job's stage comes from its events, starting with `wishlisted`.
        assert_eq!(count(kron, "latest_steps")?, 9);
        let violations: i64 =
            kron.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        assert_eq!(violations, 0);
        Ok(())
    }

    #[test]
    fn erasing_lets_the_sample_be_written_again() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let install_and_write = |write: &mut Write| {
            typeset::install_templates(write)?;
            write_sample(write)
        };
        writer.execute(install_and_write)?;
        let facts = count(writer.kron(), "facts")?;
        writer.execute(|write| {
            write.erase_everything()?;
            install_and_write(write)
        })?;
        assert_eq!(count(writer.kron(), "facts")?, facts);
        assert_eq!(count(writer.kron(), "settings")?, 1);
        Ok(())
    }
}
