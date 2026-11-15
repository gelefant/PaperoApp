use crate::error::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MscCategory {
    pub code: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
struct MscJsonRoot {
    categories: Vec<MscCategory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MscTreeNode {
    pub code: String,
    pub title: String,
    pub level: u8,
    pub confirmed_count: usize,
    pub proposed_count: usize,
    pub total_count: usize,
    pub children: Vec<MscTreeNode>,
}

static MSC_TOP_CACHE: OnceLock<Vec<MscCategory>> = OnceLock::new();

/// Returns all official 63 top-level 2-digit MSC2020 categories embedded in the binary.
pub fn all_msc_top_categories() -> &'static [MscCategory] {
    MSC_TOP_CACHE.get_or_init(|| {
        let parsed: MscJsonRoot = serde_json::from_str(include_str!("msc2020.json"))
            .expect("embedded msc2020.json must be valid JSON");
        parsed.categories
    })
}

/// Returns the official English MSC2020 category title for any code matching a known 2-digit prefix.
pub fn msc_title_for_code(code: &str) -> Option<&'static str> {
    let prefix = msc_two_digit_prefix(code)?;
    all_msc_top_categories()
        .iter()
        .find(|c| c.code == prefix)
        .map(|c| c.title.as_str())
}

/// Extracts the leading 2-digit MSC2020 section (`"00"`..`"97"`).
pub fn msc_two_digit_prefix(raw: &str) -> Option<String> {
    let trimmed = raw.trim().to_ascii_uppercase();
    let bytes = trimmed.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_digit() || !bytes[1].is_ascii_digit() {
        return None;
    }
    let code = std::str::from_utf8(&bytes[..2]).ok()?.to_string();
    if all_msc_top_categories().iter().any(|c| c.code == code) {
        Some(code)
    } else {
        None
    }
}

/// Normalizes an MSC2020 code (`"49f20"` -> `"49F20"`, `"49"` -> `"49-XX"`).
pub fn normalize_msc_code(raw: &str) -> Option<String> {
    let trimmed = raw.trim().to_ascii_uppercase();
    let two = msc_two_digit_prefix(&trimmed)?;
    if trimmed.len() == 2 {
        return Some(format!("{two}-XX"));
    }
    if trimmed.len() == 5 && trimmed.ends_with("-XX") {
        return Some(trimmed);
    }
    let cleaned: String = trimmed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if cleaned.len() >= 2 {
        Some(cleaned)
    } else {
        None
    }
}

#[derive(Default)]
struct LeafCounts {
    confirmed: usize,
    proposed: usize,
}

/// Builds the navigable MSC2020 tree anchored on the 63 official 2-digit categories,
/// populated with live confirmed/metadata and proposed document counts from SQLite.
pub fn build_msc_tree(conn: &Connection) -> Result<Vec<MscTreeNode>> {
    let mut top_counts: HashMap<String, LeafCounts> = HashMap::new();
    let mut child_counts: BTreeMap<String, BTreeMap<String, LeafCounts>> = BTreeMap::new();

    let mut stmt = conn.prepare(
        "SELECT id, msc_primary, msc_primary_status
         FROM documents
         WHERE msc_primary IS NOT NULL AND TRIM(msc_primary) != ''",
    )?;
    let primary_rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;

    // Deduplicate `(doc_id, leaf_code)` so a doc listed in primary+secondary isn't double-counted.
    let mut seen_doc_top: HashMap<(i64, String), bool> = HashMap::new();
    let mut seen_doc_leaf: HashMap<(i64, String), bool> = HashMap::new();

    for row in primary_rows {
        let (doc_id, raw_code, status) = row?;
        record_msc_hit(
            doc_id,
            &raw_code,
            status == "proposed",
            &mut seen_doc_top,
            &mut seen_doc_leaf,
        );
    }

    let mut sec_stmt = conn.prepare("SELECT document_id, msc_code, status FROM document_msc_secondary")?;
    let sec_rows = sec_stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in sec_rows {
        let (doc_id, raw_code, status) = row?;
        record_msc_hit(
            doc_id,
            &raw_code,
            status == "proposed",
            &mut seen_doc_top,
            &mut seen_doc_leaf,
        );
    }

    for ((_, top), is_prop) in seen_doc_top {
        let entry = top_counts.entry(top).or_default();
        if is_prop {
            entry.proposed += 1;
        } else {
            entry.confirmed += 1;
        }
    }

    for ((_, leaf), is_prop) in seen_doc_leaf {
        if let Some(top) = msc_two_digit_prefix(&leaf) {
            let leaf_entry = child_counts.entry(top).or_default().entry(leaf).or_default();
            if is_prop {
                leaf_entry.proposed += 1;
            } else {
                leaf_entry.confirmed += 1;
            }
        }
    }

    let mut nodes = Vec::with_capacity(all_msc_top_categories().len());
    for cat in all_msc_top_categories() {
        let tc = top_counts.get(&cat.code);
        let confirmed_count = tc.map(|x| x.confirmed).unwrap_or(0);
        let proposed_count = tc.map(|x| x.proposed).unwrap_or(0);
        let mut children = Vec::new();
        if let Some(leaves) = child_counts.get(&cat.code) {
            for (leaf_code, counts) in leaves {
                children.push(MscTreeNode {
                    code: leaf_code.clone(),
                    title: cat.title.clone(),
                    level: if leaf_code.len() >= 5 { 5 } else { 3 },
                    confirmed_count: counts.confirmed,
                    proposed_count: counts.proposed,
                    total_count: counts.confirmed + counts.proposed,
                    children: Vec::new(),
                });
            }
        }
        nodes.push(MscTreeNode {
            code: cat.code.clone(),
            title: cat.title.clone(),
            level: 2,
            confirmed_count,
            proposed_count,
            total_count: confirmed_count + proposed_count,
            children,
        });
    }

    Ok(nodes)
}

fn record_msc_hit(
    doc_id: i64,
    raw_code: &str,
    is_proposed: bool,
    seen_top: &mut HashMap<(i64, String), bool>,
    seen_leaf: &mut HashMap<(i64, String), bool>,
) {
    let Some(norm) = normalize_msc_code(raw_code) else {
        return;
    };
    let Some(top) = msc_two_digit_prefix(&norm) else {
        return;
    };
    // Confirmed beats proposed if both exist for the same doc/category.
    seen_top
        .entry((doc_id, top))
        .and_modify(|prev_prop| {
            if !is_proposed {
                *prev_prop = false;
            }
        })
        .or_insert(is_proposed);

    if !norm.ends_with("-XX") {
        seen_leaf
            .entry((doc_id, norm))
            .and_modify(|prev_prop| {
                if !is_proposed {
                    *prev_prop = false;
                }
            })
            .or_insert(is_proposed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repos::docs::create_heuristic_document;
    use crate::db::Database;
    use rusqlite::params;
    use tempfile::tempdir;

    #[test]
    fn embeds_all_63_msc2020_top_categories_and_builds_tree() {
        let cats = all_msc_top_categories();
        assert_eq!(cats.len(), 63);
        assert_eq!(msc_title_for_code("49Q20"), Some("Calculus of variations and optimal control; optimization"));

        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("msc.db"), 2).unwrap();
        let w = db.writer();
        let d1 = create_heuristic_document(&w, "Doc A").unwrap();
        let d2 = create_heuristic_document(&w, "Doc B").unwrap();
        w.execute(
            "UPDATE documents SET msc_primary = '49Q20', msc_primary_status = 'confirmed' WHERE id = ?1",
            params![d1],
        )
        .unwrap();
        w.execute(
            "UPDATE documents SET msc_primary = '49', msc_primary_status = 'proposed' WHERE id = ?1",
            params![d2],
        )
        .unwrap();

        let tree = build_msc_tree(&w).unwrap();
        let node_49 = tree.iter().find(|n| n.code == "49").unwrap();
        assert_eq!(node_49.confirmed_count, 1);
        assert_eq!(node_49.proposed_count, 1);
        assert_eq!(node_49.children.len(), 1);
        assert_eq!(node_49.children[0].code, "49Q20");
    }
}
