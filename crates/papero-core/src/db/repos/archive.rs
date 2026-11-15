use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualMetadataSnapshot {
    pub work_type: String,
    pub title: String,
    pub title_norm: String,
    pub authors_norm_summary: String,
    pub journal_abbrev: Option<String>,
    pub journal_full: Option<String>,
    pub booktitle: Option<String>,
    pub series: Option<String>,
    pub volume: Option<String>,
    pub issue_number: Option<String>,
    pub year: Option<i64>,
    pub pages: Option<String>,
    pub edition: Option<String>,
    pub publisher: Option<String>,
    pub address: Option<String>,
    pub isbn: Option<String>,
    pub issn: Option<String>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub arxiv_version: Option<String>,
    pub mr_number: Option<String>,
    pub mr_number_display: Option<String>,
    pub mr_reviewer: Option<String>,
    pub zbl_number: Option<String>,
    pub url: Option<String>,
    pub msc_primary: Option<String>,
    pub msc_primary_status: String,
    pub abstract_text: Option<String>,
    pub manual_locked_fields: Vec<String>,
    #[serde(default)]
    pub metadata_verified: bool,
    pub authors: Vec<ArchivedAuthor>,
    pub msc_secondary: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedAuthor {
    pub position: i64,
    pub family_name: String,
    pub given_name: String,
    pub name_norm: String,
    pub role: String,
}

#[derive(Debug, Clone)]
pub struct RestoredManualArchive {
    pub snapshot: ManualMetadataSnapshot,
    pub tags: Vec<String>,
}

/// Snapshots a document's manual metadata & tags into `manual_metadata_archive`
/// keyed by `blake3_hash` if the document has any manually locked fields,
/// confirmed MSC, or tags.
pub fn archive_if_manual(conn: &Connection, document_id: i64, blake3_hash: &str) -> Result<bool> {
    let snapshot_opt: Option<ManualMetadataSnapshot> = conn
        .query_row(
            "SELECT work_type, title, title_norm, authors_norm_summary,
                    journal_abbrev, journal_full, booktitle, series, volume, issue_number,
                    year, pages, edition, publisher, address, isbn, issn, doi,
                    arxiv_id, arxiv_version, mr_number, mr_number_display, mr_reviewer,
                    zbl_number, url, msc_primary, msc_primary_status, abstract_text,
                    manual_locked_fields, metadata_verified
             FROM documents WHERE id = ?1",
            params![document_id],
            |r| {
                let locked_raw: String = r.get(28)?;
                let locked: Vec<String> = serde_json::from_str(&locked_raw).unwrap_or_default();
                Ok(ManualMetadataSnapshot {
                    work_type: r.get(0)?,
                    title: r.get(1)?,
                    title_norm: r.get(2)?,
                    authors_norm_summary: r.get(3)?,
                    journal_abbrev: r.get(4)?,
                    journal_full: r.get(5)?,
                    booktitle: r.get(6)?,
                    series: r.get(7)?,
                    volume: r.get(8)?,
                    issue_number: r.get(9)?,
                    year: r.get(10)?,
                    pages: r.get(11)?,
                    edition: r.get(12)?,
                    publisher: r.get(13)?,
                    address: r.get(14)?,
                    isbn: r.get(15)?,
                    issn: r.get(16)?,
                    doi: r.get(17)?,
                    arxiv_id: r.get(18)?,
                    arxiv_version: r.get(19)?,
                    mr_number: r.get(20)?,
                    mr_number_display: r.get(21)?,
                    mr_reviewer: r.get(22)?,
                    zbl_number: r.get(23)?,
                    url: r.get(24)?,
                    msc_primary: r.get(25)?,
                    msc_primary_status: r.get(26)?,
                    abstract_text: r.get(27)?,
                    manual_locked_fields: locked,
                    metadata_verified: r.get::<_, i64>(29)? != 0,
                    authors: Vec::new(),
                    msc_secondary: Vec::new(),
                })
            },
        )
        .optional()?;

    let Some(mut snapshot) = snapshot_opt else {
        return Ok(false);
    };

    let mut tag_stmt = conn.prepare(
        "SELECT t.name FROM tags t
         JOIN document_tags dt ON dt.tag_id = t.id
         WHERE dt.document_id = ?1
         ORDER BY t.name ASC",
    )?;
    let tags: Vec<String> = tag_stmt
        .query_map(params![document_id], |r| r.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let has_confirmed_msc = snapshot.msc_primary_status == "confirmed";
    if snapshot.manual_locked_fields.is_empty()
        && tags.is_empty()
        && !has_confirmed_msc
        && !snapshot.metadata_verified
    {
        return Ok(false);
    }

    let mut auth_stmt = conn.prepare(
        "SELECT position, family_name, given_name, name_norm, role
         FROM document_authors WHERE document_id = ?1 ORDER BY role, position",
    )?;
    snapshot.authors = auth_stmt
        .query_map(params![document_id], |r| {
            Ok(ArchivedAuthor {
                position: r.get(0)?,
                family_name: r.get(1)?,
                given_name: r.get(2)?,
                name_norm: r.get(3)?,
                role: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut msc_stmt = conn.prepare(
        "SELECT msc_code, status FROM document_msc_secondary WHERE document_id = ?1",
    )?;
    snapshot.msc_secondary = msc_stmt
        .query_map(params![document_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let fields_json = serde_json::to_string(&snapshot)?;
    let tags_json = serde_json::to_string(&tags)?;
    let now = chrono::Utc::now().timestamp();

    conn.execute(
        "INSERT INTO manual_metadata_archive (blake3_hash, manual_fields_json, tags_json, archived_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(blake3_hash) DO UPDATE SET
            manual_fields_json = excluded.manual_fields_json,
            tags_json = excluded.tags_json,
            archived_at = excluded.archived_at",
        params![blake3_hash, fields_json, tags_json, now],
    )?;

    Ok(true)
}

pub fn load_archive(
    conn: &Connection,
    blake3_hash: &str,
) -> Result<Option<RestoredManualArchive>> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT manual_fields_json, tags_json FROM manual_metadata_archive WHERE blake3_hash = ?1",
            params![blake3_hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    let Some((fields_json, tags_json)) = row else {
        return Ok(None);
    };

    let snapshot: ManualMetadataSnapshot = serde_json::from_str(&fields_json)?;
    let tags: Vec<String> = serde_json::from_str(&tags_json)?;
    Ok(Some(RestoredManualArchive { snapshot, tags }))
}

pub fn apply_restored_archive(
    conn: &Connection,
    document_id: i64,
    archive: &RestoredManualArchive,
) -> Result<()> {
    let s = &archive.snapshot;
    let locked_json = serde_json::to_string(&s.manual_locked_fields)?;
    let now = chrono::Utc::now().timestamp();

    conn.execute(
        "UPDATE documents SET
            work_type = ?1, title = ?2, title_norm = ?3, authors_norm_summary = ?4,
            journal_abbrev = ?5, journal_full = ?6, booktitle = ?7, series = ?8,
            volume = ?9, issue_number = ?10, year = ?11, pages = ?12, edition = ?13,
            publisher = ?14, address = ?15, isbn = ?16, issn = ?17, doi = ?18,
            arxiv_id = ?19, arxiv_version = ?20, mr_number = ?21, mr_number_display = ?22,
            mr_reviewer = ?23, zbl_number = ?24, url = ?25, msc_primary = ?26,
            msc_primary_status = ?27, abstract_text = ?28, manual_locked_fields = ?29,
            metadata_verified = ?30,
            metadata_source = 'manual', updated_at = ?31
         WHERE id = ?32",
        params![
            s.work_type,
            s.title,
            s.title_norm,
            s.authors_norm_summary,
            s.journal_abbrev,
            s.journal_full,
            s.booktitle,
            s.series,
            s.volume,
            s.issue_number,
            s.year,
            s.pages,
            s.edition,
            s.publisher,
            s.address,
            s.isbn,
            s.issn,
            s.doi,
            s.arxiv_id,
            s.arxiv_version,
            s.mr_number,
            s.mr_number_display,
            s.mr_reviewer,
            s.zbl_number,
            s.url,
            s.msc_primary,
            s.msc_primary_status,
            s.abstract_text,
            locked_json,
            s.metadata_verified as i64,
            now,
            document_id
        ],
    )?;

    if !s.authors.is_empty() {
        conn.execute(
            "DELETE FROM document_authors WHERE document_id = ?1",
            params![document_id],
        )?;
        for a in &s.authors {
            conn.execute(
                "INSERT INTO document_authors (document_id, position, family_name, given_name, name_norm, role)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![document_id, a.position, a.family_name, a.given_name, a.name_norm, a.role],
            )?;
        }
    }

    for (code, status) in &s.msc_secondary {
        conn.execute(
            "INSERT OR IGNORE INTO document_msc_secondary (document_id, msc_code, status)
             VALUES (?1, ?2, ?3)",
            params![document_id, code, status],
        )?;
    }

    for tag in &archive.tags {
        conn.execute(
            "INSERT OR IGNORE INTO tags (name) VALUES (?1)",
            params![tag],
        )?;
        let tag_id: i64 = conn.query_row(
            "SELECT id FROM tags WHERE name = ?1",
            params![tag],
            |r| r.get(0),
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO document_tags (document_id, tag_id) VALUES (?1, ?2)",
            params![document_id, tag_id],
        )?;
    }

    Ok(())
}
