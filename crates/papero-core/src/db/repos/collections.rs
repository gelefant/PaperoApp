use crate::error::{CoreError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionView {
    pub id: i64,
    pub name: String,
    pub color_hex: Option<String>,
    pub sort_order: i64,
    pub document_count: usize,
    pub document_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSearchView {
    pub id: i64,
    pub name: String,
    pub search_mode: String,
    pub query_text: String,
    pub filter_json: String,
    pub sort_order: i64,
    pub created_at: i64,
}

pub fn list_collections(conn: &Connection) -> Result<Vec<CollectionView>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, color_hex, sort_order
         FROM collections
         ORDER BY sort_order ASC, id ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(CollectionView {
            id: r.get(0)?,
            name: r.get(1)?,
            color_hex: r.get(2)?,
            sort_order: r.get(3)?,
            document_count: 0,
            document_ids: Vec::new(),
        })
    })?;
    let mut list: Vec<CollectionView> = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    if list.is_empty() {
        return Ok(list);
    }

    let mut member_stmt = conn.prepare(
        "SELECT cd.collection_id, cd.document_id
         FROM collection_documents cd
         JOIN documents d ON d.id = cd.document_id
         ORDER BY cd.collection_id ASC, cd.position ASC, cd.added_at DESC",
    )?;
    let member_rows =
        member_stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    let mut by_col: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
    for mr in member_rows {
        let (cid, did) = mr?;
        by_col.entry(cid).or_default().push(did);
    }

    for col in &mut list {
        if let Some(ids) = by_col.remove(&col.id) {
            col.document_count = ids.len();
            col.document_ids = ids;
        }
    }
    Ok(list)
}

pub fn create_collection(
    conn: &Connection,
    name: &str,
    color_hex: Option<&str>,
) -> Result<CollectionView> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CoreError::Config("Collection name cannot be empty".into()));
    }
    let next_order: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM collections",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO collections (name, color_hex, sort_order, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![trimmed, color_hex, next_order, now],
    )?;
    let id = conn.last_insert_rowid();
    Ok(CollectionView {
        id,
        name: trimmed.to_string(),
        color_hex: color_hex.map(ToOwned::to_owned),
        sort_order: next_order,
        document_count: 0,
        document_ids: Vec::new(),
    })
}

pub fn rename_collection(conn: &Connection, collection_id: i64, name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CoreError::Config("Collection name cannot be empty".into()));
    }
    let now = chrono::Utc::now().timestamp();
    let updated = conn.execute(
        "UPDATE collections SET name = ?1, updated_at = ?2 WHERE id = ?3",
        params![trimmed, now, collection_id],
    )?;
    if updated == 0 {
        return Err(CoreError::NotFound(format!("collection {collection_id}")));
    }
    Ok(())
}

pub fn delete_collection(conn: &Connection, collection_id: i64) -> Result<()> {
    conn.execute("DELETE FROM collections WHERE id = ?1", params![collection_id])?;
    Ok(())
}

pub fn reorder_collections(conn: &Connection, ordered_ids: &[i64]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare_cached("UPDATE collections SET sort_order = ?1 WHERE id = ?2")?;
        for (idx, &cid) in ordered_ids.iter().enumerate() {
            stmt.execute(params![idx as i64, cid])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn add_documents_to_collection(
    conn: &Connection,
    collection_id: i64,
    document_ids: &[i64],
) -> Result<usize> {
    if document_ids.is_empty() {
        return Ok(0);
    }
    let exists: Option<i64> = conn
        .query_row(
            "SELECT id FROM collections WHERE id = ?1",
            params![collection_id],
            |r| r.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Err(CoreError::NotFound(format!("collection {collection_id}")));
    }
    let mut next_pos: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM collection_documents WHERE collection_id = ?1",
            params![collection_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let now = chrono::Utc::now().timestamp();
    let tx = conn.unchecked_transaction()?;
    let mut added = 0usize;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT OR IGNORE INTO collection_documents (collection_id, document_id, position, added_at)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for &doc_id in document_ids {
            let n = stmt.execute(params![collection_id, doc_id, next_pos, now])?;
            if n > 0 {
                added += 1;
                next_pos += 1;
            }
        }
    }
    tx.commit()?;
    Ok(added)
}

pub fn remove_documents_from_collection(
    conn: &Connection,
    collection_id: i64,
    document_ids: &[i64],
) -> Result<usize> {
    if document_ids.is_empty() {
        return Ok(0);
    }
    let tx = conn.unchecked_transaction()?;
    let mut removed = 0usize;
    {
        let mut stmt = tx.prepare_cached(
            "DELETE FROM collection_documents WHERE collection_id = ?1 AND document_id = ?2",
        )?;
        for &doc_id in document_ids {
            removed += stmt.execute(params![collection_id, doc_id])?;
        }
    }
    tx.commit()?;
    Ok(removed)
}

pub fn list_saved_searches(conn: &Connection) -> Result<Vec<SavedSearchView>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, search_mode, query_text, filter_json, sort_order, created_at
         FROM saved_searches
         ORDER BY sort_order ASC, id ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(SavedSearchView {
            id: r.get(0)?,
            name: r.get(1)?,
            search_mode: r.get(2)?,
            query_text: r.get(3)?,
            filter_json: r.get(4)?,
            sort_order: r.get(5)?,
            created_at: r.get(6)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn create_saved_search(
    conn: &Connection,
    name: &str,
    search_mode: &str,
    query_text: &str,
    filter_json: &str,
) -> Result<SavedSearchView> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CoreError::Config("Saved view name cannot be empty".into()));
    }
    let mode = if search_mode.trim().eq_ignore_ascii_case("content") {
        "content"
    } else {
        "metadata"
    };
    let next_order: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM saved_searches",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO saved_searches (name, search_mode, query_text, filter_json, sort_order, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![trimmed, mode, query_text.trim(), filter_json, next_order, now],
    )?;
    let id = conn.last_insert_rowid();
    Ok(SavedSearchView {
        id,
        name: trimmed.to_string(),
        search_mode: mode.to_string(),
        query_text: query_text.trim().to_string(),
        filter_json: filter_json.to_string(),
        sort_order: next_order,
        created_at: now,
    })
}

pub fn delete_saved_search(conn: &Connection, saved_search_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM saved_searches WHERE id = ?1",
        params![saved_search_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repos::docs::create_heuristic_document;
    use crate::db::Database;

    #[test]
    fn collections_and_saved_searches_crud_and_reorder() {
        let db = Database::in_memory().unwrap();
        let w = db.writer();
        let d1 = create_heuristic_document(&w, "Doc A").unwrap();
        let d2 = create_heuristic_document(&w, "Doc B").unwrap();

        let c1 = create_collection(&w, "Paper X", Some("#2563eb")).unwrap();
        let c2 = create_collection(&w, "Corso Y", None).unwrap();
        assert_eq!(add_documents_to_collection(&w, c1.id, &[d1, d2]).unwrap(), 2);
        assert_eq!(add_documents_to_collection(&w, c1.id, &[d1]).unwrap(), 0);

        reorder_collections(&w, &[c2.id, c1.id]).unwrap();
        let cols = list_collections(&w).unwrap();
        assert_eq!(cols[0].id, c2.id);
        assert_eq!(cols[1].id, c1.id);
        assert_eq!(cols[1].document_count, 2);

        rename_collection(&w, c2.id, "Corso Analisi").unwrap();
        assert_eq!(list_collections(&w).unwrap()[0].name, "Corso Analisi");

        assert_eq!(remove_documents_from_collection(&w, c1.id, &[d2]).unwrap(), 1);
        assert_eq!(list_collections(&w).unwrap()[1].document_count, 1);

        let ss = create_saved_search(&w, "PDE 2020+", "metadata", "", r#"{"year_from":2020}"#).unwrap();
        assert_eq!(list_saved_searches(&w).unwrap().len(), 1);
        delete_saved_search(&w, ss.id).unwrap();
        assert!(list_saved_searches(&w).unwrap().is_empty());
    }
}
