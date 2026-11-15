use crate::config::{normalize_metadata_source_priority, AppConfig};
use crate::credentials::load_polite_email;
use crate::db::repos::archive::archive_if_manual;
use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::extract::djvu::extract_djvu_front_matter;
use crate::extract::pdf::extract_pdf_front_matter;
use crate::fs::roots::resolve_rel_path;
use crate::metadata::arxiv::ArxivClient;
use crate::metadata::crossref::CrossrefClient;
use crate::metadata::datacite::DataCiteClient;
use crate::metadata::dblp::DblpClient;
use crate::metadata::europepmc::EuropePmcClient;
use crate::metadata::extract::{
    extract_identifiers_and_heuristics_with_front_pages, extract_prose_core_title,
    looks_like_garbage_title, strip_inline_markup_tags,
};
use crate::metadata::inspirehep::InspireHepClient;
use crate::metadata::models::{
    clean_author_surname, clean_metadata_text, normalize_doi, normalize_mr_digits, normalize_pages,
    BibliographicAuthor, BibliographicRecord, CandidateMatch, CandidateRow, DocumentMetadataView,
    ExtractedIdentifiers, WorkType,
};
use crate::metadata::mrlookup::{
    compute_title_similarity, is_erratum_or_review_mismatch, normalize_title_words, pair_title_sim,
    significant_title_tokens, significant_title_words_vec, titles_represent_same_work,
    MrLookupClient,
};
use crate::metadata::openalex::OpenAlexClient;
use crate::metadata::openlibrary::OpenLibraryClient;
use crate::metadata::semanticscholar::SemanticScholarClient;
use crate::metadata::zbmath::ZbMathClient;
use crate::text_norm::normalize_math_text;
use futures_util::FutureExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataLookupScope {
    SingleRecordUserAction,
    BulkBackgroundScan,
}

#[derive(Debug)]
pub struct WaterfallClients {
    pub mrlookup: MrLookupClient,
    pub zbmath: ZbMathClient,
    pub crossref: CrossrefClient,
    pub dblp: DblpClient,
    pub openalex: OpenAlexClient,
    pub datacite: DataCiteClient,
    pub inspirehep: InspireHepClient,
    pub europepmc: EuropePmcClient,
    pub semanticscholar: SemanticScholarClient,
    pub arxiv: ArxivClient,
    pub openlibrary: OpenLibraryClient,
}

impl WaterfallClients {
    pub fn from_config(config: &AppConfig) -> Result<Self> {
        const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
        let email = load_polite_email(config);
        let ua = match email.as_deref() {
            Some(mail) => format!("Papero/{APP_VERSION} ({mail})"),
            None => format!("Papero/{APP_VERSION}"),
        };
        Ok(Self {
            mrlookup: MrLookupClient::new(ua.clone())?,
            zbmath: ZbMathClient::new(ua.clone())?,
            crossref: CrossrefClient::new(APP_VERSION, email.clone())?,
            dblp: DblpClient::new(ua.clone())?,
            openalex: OpenAlexClient::new(APP_VERSION, email.clone())?,
            datacite: DataCiteClient::new(ua.clone())?,
            inspirehep: InspireHepClient::new(ua.clone())?,
            europepmc: EuropePmcClient::new(ua.clone())?,
            semanticscholar: SemanticScholarClient::new(ua.clone())?,
            arxiv: ArxivClient::new(ua)?,
            openlibrary: OpenLibraryClient::new(APP_VERSION, email)?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveOutcome {
    pub document_id: i64,
    pub status: String, // "updated" | "candidates_found" | "offline_only" | "verified_locked" | "unchanged"
    pub source: Option<String>,
    pub candidates_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveAndFetchResult {
    pub outcome: ResolveOutcome,
    pub metadata: DocumentMetadataView,
}

pub fn is_document_metadata_verified(conn: &Connection, document_id: i64) -> Result<bool> {
    let flag: i64 = conn.query_row(
        "SELECT COALESCE(metadata_verified, 0) FROM documents WHERE id = ?1",
        params![document_id],
        |r| r.get(0),
    )?;
    Ok(flag != 0)
}

/// Applies local extracted identifiers/heuristics (always respecting `manual_locked_fields`
/// and never modifying documents whose metadata has been confirmed/verified)
/// without making any network requests.
pub fn apply_extracted_identifiers_to_doc(
    conn: &Connection,
    document_id: i64,
    ids: &ExtractedIdentifiers,
    work_type_hint: Option<WorkType>,
) -> Result<()> {
    let (cur_source, cur_title, verified): (Option<String>, String, i64) = conn.query_row(
        "SELECT metadata_source, title, COALESCE(metadata_verified, 0) FROM documents WHERE id = ?1",
        params![document_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if verified != 0 {
        return Ok(());
    }

    let locked = load_locked_fields(conn, document_id)?;
    let now = chrono::Utc::now().timestamp();

    let is_heuristic_doc = cur_source.as_deref().unwrap_or("heuristic") == "heuristic";
    let can_update_heuristic_title = is_heuristic_doc && !locked.contains("title");
    let new_title = if can_update_heuristic_title {
        ids.heuristic_title
            .as_deref()
            .filter(|s| s.len() >= 4)
            .unwrap_or(&cur_title)
            .to_string()
    } else {
        cur_title
    };
    let new_title_norm = normalize_math_text(&new_title);
    let wt = work_type_hint
        .unwrap_or(if ids.looks_like_book { WorkType::Book } else { WorkType::Article })
        .as_str();

    conn.execute(
        "UPDATE documents SET
            work_type = CASE WHEN COALESCE(metadata_source, 'heuristic') = 'heuristic' AND ?2 = 0 THEN ?3 ELSE work_type END,
            title = ?4,
            title_norm = ?5,
            doi = CASE
                WHEN ?6 != 0 THEN doi
                WHEN COALESCE(metadata_source, 'heuristic') = 'heuristic' THEN ?7
                ELSE COALESCE(doi, ?7)
            END,
            arxiv_id = CASE
                WHEN ?8 != 0 THEN arxiv_id
                WHEN COALESCE(metadata_source, 'heuristic') = 'heuristic' THEN ?9
                ELSE COALESCE(arxiv_id, ?9)
            END,
            arxiv_version = CASE
                WHEN ?8 != 0 THEN arxiv_version
                WHEN COALESCE(metadata_source, 'heuristic') = 'heuristic' THEN ?10
                ELSE COALESCE(arxiv_version, ?10)
            END,
            isbn = CASE
                WHEN ?11 != 0 THEN isbn
                WHEN COALESCE(metadata_source, 'heuristic') = 'heuristic' THEN ?12
                ELSE COALESCE(isbn, ?12)
            END,
            mr_number = CASE
                WHEN ?13 != 0 THEN mr_number
                WHEN COALESCE(metadata_source, 'heuristic') = 'heuristic' THEN ?14
                ELSE COALESCE(mr_number, ?14)
            END,
            zbl_number = CASE
                WHEN ?15 != 0 THEN zbl_number
                WHEN COALESCE(metadata_source, 'heuristic') = 'heuristic' THEN ?16
                ELSE COALESCE(zbl_number, ?16)
            END,
            year = CASE
                WHEN ?17 != 0 THEN year
                WHEN COALESCE(metadata_source, 'heuristic') = 'heuristic' THEN COALESCE(?18, year)
                ELSE COALESCE(year, ?18)
            END,
            updated_at = ?19
         WHERE id = ?1",
        params![
            document_id,
            locked.contains("work_type") as i64,
            wt,
            new_title,
            new_title_norm,
            locked.contains("doi") as i64,
            ids.doi,
            locked.contains("arxiv_id") as i64,
            ids.arxiv_id,
            ids.arxiv_version,
            locked.contains("isbn") as i64,
            ids.isbn,
            locked.contains("mr_number") as i64,
            ids.mr_number,
            locked.contains("zbl_number") as i64,
            ids.zbl_number,
            locked.contains("year") as i64,
            ids.heuristic_year,
            now
        ],
    )?;

    if !locked.contains("authors") {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM document_authors WHERE document_id = ?1",
            params![document_id],
            |r| r.get(0),
        )?;
        let mut author_list: Vec<&str> = ids
            .heuristic_authors
            .iter()
            .map(String::as_str)
            .filter(|s| !s.trim().is_empty())
            .collect();
        if author_list.is_empty() {
            if let Some(au) = ids
                .heuristic_first_author
                .as_deref()
                .filter(|s| !s.trim().is_empty())
            {
                author_list.push(au);
            }
        }
        if !author_list.is_empty() && (count == 0 || is_heuristic_doc) {
            conn.execute(
                "DELETE FROM document_authors WHERE document_id = ?1",
                params![document_id],
            )?;
            let mut summary_parts = Vec::with_capacity(author_list.len());
            for (pos, raw_au) in author_list.iter().enumerate() {
                let parsed = split_heuristic_author(raw_au);
                let norm = normalize_math_text(&format!("{} {}", parsed.given_name, parsed.family_name));
                if !norm.is_empty() {
                    summary_parts.push(norm.clone());
                }
                conn.execute(
                    "INSERT INTO document_authors (document_id, position, family_name, given_name, name_norm, role)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'author')",
                    params![document_id, pos as i64, parsed.family_name, parsed.given_name, norm],
                )?;
            }
            let summary = summary_parts.join("; ");
            conn.execute(
                "UPDATE documents SET authors_norm_summary = ?1 WHERE id = ?2",
                params![summary, document_id],
            )?;
        }
    }

    Ok(())
}

/// Resolves metadata for a document via the configurable waterfall:
/// 1. MRLookup (only if allowed by scope & settings)
/// 2. zbMATH Open (CC-BY-SA 4.0)
/// 3. Crossref (polite/public adaptive)
/// 4. arXiv API
/// 5. Open Library (books only, strict 1 req/s cached)
pub async fn resolve_document_metadata(
    db: &Database,
    clients: &WaterfallClients,
    config: &AppConfig,
    document_id: i64,
    scope: MetadataLookupScope,
) -> Result<ResolveOutcome> {
    match AssertUnwindSafe(resolve_document_metadata_inner(
        db,
        clients,
        config,
        document_id,
        scope,
    ))
    .catch_unwind()
    .await
    {
        Ok(res) => res,
        Err(_) => {
            tracing::error!(
                document_id,
                "unexpected panic while resolving bibliographic metadata"
            );
            Err(CoreError::Network(format!(
                "internal error while resolving metadata for document {document_id}"
            )))
        }
    }
}

async fn resolve_document_metadata_inner(
    db: &Database,
    clients: &WaterfallClients,
    config: &AppConfig,
    document_id: i64,
    scope: MetadataLookupScope,
) -> Result<ResolveOutcome> {
    let verified = {
        let r = db.reader();
        is_document_metadata_verified(&r, document_id)?
    };
    if verified {
        let w = db.writer();
        let _ = clear_metadata_jobs_for_doc(&w, document_id);
        return Ok(ResolveOutcome {
            document_id,
            status: "verified_locked".to_string(),
            source: None,
            candidates_count: 0,
        });
    }

    let ctx = gather_document_context(db, config, document_id)?;
    let mut working_ids = ctx.ids.clone();
    {
        let w = db.writer();
        apply_extracted_identifiers_to_doc(&w, document_id, &working_ids, None)?;
    }

    if !config.network_lookups_enabled {
        return Ok(ResolveOutcome {
            document_id,
            status: "offline_only".to_string(),
            source: None,
            candidates_count: 0,
        });
    }

    let allow_mrlookup = match scope {
        MetadataLookupScope::SingleRecordUserAction => config.mrlookup_enabled_single,
        MetadataLookupScope::BulkBackgroundScan => config.mrlookup_enabled_bulk,
    };

    let mut all_candidates: Vec<CandidateMatch> = Vec::new();
    let mut best_certain: Option<CandidateMatch> = None;

    let ordered_sources = normalize_metadata_source_priority(&config.metadata_source_priority);

    // Identifier Bridge (Zotero-inspired, zero external non-public services):
    // When a PDF has an explicit DOI, Elsevier PII, or arXiv ID on front pages/filename, resolve its
    // exact canonical Title + Author first so prioritized math sources without DOI endpoints (like MRLookup #1)
    // receive clean queries AND any footnote-citation ID failing front-matter verification is discarded.
    let mut bridge_record: Option<BibliographicRecord> = None;
    let mut bridge_hit: Option<CandidateMatch> = None;
    if let Some(doi_val) = working_ids
        .doi
        .clone()
        .filter(|d| !d.starts_with("10.48550/"))
    {
        let mut doi_hit = if config.crossref_enabled {
            clients
                .crossref
                .lookup_exact_doi(db, &doi_val)
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        if doi_hit.is_none() && config.openalex_enabled {
            doi_hit = clients
                .openalex
                .lookup_exact_doi(db, &doi_val)
                .await
                .ok()
                .flatten();
        }
        if doi_hit.is_none() && (config.crossref_enabled || config.datacite_enabled) {
            doi_hit = clients
                .crossref
                .lookup_doi_content_negotiation(db, &doi_val)
                .await
                .ok()
                .flatten();
        }
        if let Some(hit) = doi_hit {
            if verify_candidate_against_context(&hit, &ctx) {
                enrich_working_ids_from_bridge(&mut working_ids, &hit.record, &ctx);
                bridge_record = Some(hit.record.clone());
                bridge_hit = Some(hit);
            } else if !ctx.is_guided {
                // The extracted DOI belonged to a cited paper (e.g. page-1 footnote) rather than this PDF!
                working_ids.doi = None;
                let w = db.writer();
                let _ = w.execute(
                    "UPDATE documents SET doi = NULL WHERE id = ?1 AND COALESCE(metadata_source, 'heuristic') = 'heuristic'",
                    params![document_id],
                );
            }
        }
    }

    if bridge_record.is_none() && config.crossref_enabled {
        if let Some(pii_val) = working_ids.pii.clone() {
            if let Ok(Some(pii_hit)) = clients.crossref.lookup_alternative_id(db, &pii_val).await {
                if verify_candidate_against_context(&pii_hit, &ctx) {
                    enrich_working_ids_from_bridge(&mut working_ids, &pii_hit.record, &ctx);
                    if working_ids.doi.is_none() {
                        working_ids.doi = pii_hit.record.doi.clone();
                    }
                    bridge_record = Some(pii_hit.record.clone());
                    bridge_hit = Some(pii_hit);
                } else if !ctx.is_guided {
                    working_ids.pii = None;
                }
            }
        }
    }

    if bridge_record.is_none() && config.arxiv_enabled {
        if let Some(ax_id) = working_ids.arxiv_id.clone() {
            let ax_only = ExtractedIdentifiers {
                arxiv_id: Some(ax_id),
                ..ExtractedIdentifiers::default()
            };
            if let Ok(ax_hits) = clients.arxiv.lookup(db, &ax_only).await {
                if let Some(top_ax) = ax_hits.into_iter().next().filter(|h| h.score >= 0.95) {
                    if verify_candidate_against_context(&top_ax, &ctx) {
                        enrich_working_ids_from_bridge(&mut working_ids, &top_ax.record, &ctx);
                        if working_ids
                            .doi
                            .as_deref()
                            .is_none_or(|d| d.starts_with("10.48550/"))
                        {
                            if let Some(pub_doi) = top_ax
                                .record
                                .doi
                                .as_ref()
                                .filter(|d| !d.starts_with("10.48550/"))
                            {
                                working_ids.doi = Some(pub_doi.clone());
                            }
                        }
                        bridge_record = Some(top_ax.record.clone());
                        bridge_hit = Some(top_ax);
                    } else if !ctx.is_guided {
                        working_ids.arxiv_id = None;
                        working_ids.arxiv_version = None;
                    }
                }
            }
        }
    }

    run_waterfall_pass(
        db,
        clients,
        config,
        allow_mrlookup,
        &ordered_sources,
        &working_ids,
        &ctx,
        &mut best_certain,
        &mut all_candidates,
    )
    .await;

    // Multi-hypothesis title fallback: if Pass 1 did not yield a confident match (e.g. primary title
    // had mangled PDF math symbols, subtitle noise, or differed from embedded/filename title), try up to
    // 2 alternative title hypotheses across the prioritized waterfall.
    if best_certain.is_none() && bridge_record.is_none() && !working_ids.alt_heuristic_titles.is_empty() {
        let alt_list: Vec<String> = working_ids
            .alt_heuristic_titles
            .iter()
            .take(2)
            .cloned()
            .collect();
        for alt_ti in alt_list {
            if best_certain.is_some() {
                break;
            }
            let mut alt_ids = working_ids.clone();
            alt_ids.heuristic_title = Some(alt_ti);
            run_waterfall_pass(
                db,
                clients,
                config,
                allow_mrlookup,
                &ordered_sources,
                &alt_ids,
                &ctx,
                &mut best_certain,
                &mut all_candidates,
            )
            .await;
        }
    }

    if best_certain.is_none() {
        if let Some(bh) = bridge_hit {
            best_certain = Some(bh);
        }
    }

    // Final title-only double-check: if extracted authors/year/IDs/keywords polluted queries or
    // `documents.title` held a clean title, retry enabled sources with ONLY the title and accept
    // only an unambiguous total title match verified against front-matter context.
    if best_certain.is_none() {
        run_final_title_only_double_check(
            db,
            clients,
            config,
            allow_mrlookup,
            &ordered_sources,
            &working_ids,
            &ctx,
            &mut best_certain,
            &mut all_candidates,
        )
        .await;
    }

    if let Some(mut chosen) = best_certain {
        if let Some(ref bridge) = bridge_record {
            if is_same_work(&chosen.record, bridge) {
                enrich_record_from_same_work(&mut chosen.record, bridge);
            }
        }
        for cand in &all_candidates {
            if cand.score >= 0.85
                && verify_candidate_against_context(cand, &ctx)
                && is_same_work(&chosen.record, &cand.record)
            {
                enrich_record_from_same_work(&mut chosen.record, &cand.record);
            }
        }
        let w = db.writer();
        apply_record_respecting_locks(&w, document_id, &chosen.record, false)?;
        w.execute(
            "DELETE FROM metadata_candidates WHERE document_id = ?1",
            params![document_id],
        )?;
        return Ok(ResolveOutcome {
            document_id,
            status: "updated".to_string(),
            source: Some(chosen.source),
            candidates_count: 0,
        });
    }

    let rank_of = |src: &str| -> usize {
        ordered_sources
            .iter()
            .position(|s| s == src)
            .unwrap_or(ordered_sources.len())
    };
    all_candidates.retain(|c| c.score >= 0.45);
    all_candidates.sort_by(|a, b| {
        let diff = (b.score - a.score).abs();
        if diff <= 0.02 && is_same_work(&a.record, &b.record) {
            record_completeness_score(&b.record)
                .cmp(&record_completeness_score(&a.record))
                .then_with(|| rank_of(&a.source).cmp(&rank_of(&b.source)))
                .then_with(|| b.score.total_cmp(&a.score))
        } else if diff <= 0.015 {
            rank_of(&a.source)
                .cmp(&rank_of(&b.source))
                .then_with(|| b.score.total_cmp(&a.score))
        } else {
            b.score.total_cmp(&a.score)
        }
    });
    all_candidates.truncate(8);

    if !all_candidates.is_empty() {
        let now = chrono::Utc::now().timestamp();
        let w = db.writer();
        let tx = w.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM metadata_candidates WHERE document_id = ?1",
            params![document_id],
        )?;
        for cand in &all_candidates {
            let json = serde_json::to_string(&cand.record)?;
            tx.execute(
                "INSERT INTO metadata_candidates (document_id, source, match_score, candidate_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![document_id, cand.source, cand.score as f64, json, now],
            )?;
        }
        tx.commit()?;
        return Ok(ResolveOutcome {
            document_id,
            status: "candidates_found".to_string(),
            source: None,
            candidates_count: all_candidates.len(),
        });
    }

    Ok(ResolveOutcome {
        document_id,
        status: "unchanged".to_string(),
        source: None,
        candidates_count: 0,
    })
}

async fn fetch_source_hits(
    db: &Database,
    clients: &WaterfallClients,
    config: &AppConfig,
    allow_mrlookup: bool,
    src: &str,
    ids: &ExtractedIdentifiers,
) -> Vec<CandidateMatch> {
    let res = match src {
        "mrlookup" if allow_mrlookup => clients.mrlookup.lookup(db, ids).await,
        "zbmath" if config.zbmath_enabled => clients.zbmath.lookup(db, ids).await,
        "crossref" if config.crossref_enabled => clients.crossref.lookup(db, ids).await,
        "dblp" if config.dblp_enabled => clients.dblp.lookup(db, ids).await,
        "openalex" if config.openalex_enabled => clients.openalex.lookup(db, ids).await,
        "inspirehep" if config.inspirehep_enabled => clients.inspirehep.lookup(db, ids).await,
        "arxiv" if config.arxiv_enabled => clients.arxiv.lookup(db, ids).await,
        "datacite" if config.datacite_enabled => clients.datacite.lookup(db, ids).await,
        "semanticscholar" if config.semanticscholar_enabled => {
            clients.semanticscholar.lookup(db, ids).await
        }
        "europepmc" if config.europepmc_enabled => clients.europepmc.lookup(db, ids).await,
        "openlibrary"
            if config.openlibrary_enabled && (ids.looks_like_book || ids.isbn.is_some()) =>
        {
            clients.openlibrary.lookup_book(db, ids).await
        }
        _ => return Vec::new(),
    };
    res.unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
async fn run_waterfall_pass(
    db: &Database,
    clients: &WaterfallClients,
    config: &AppConfig,
    allow_mrlookup: bool,
    ordered_sources: &[String],
    working_ids: &ExtractedIdentifiers,
    ctx: &GatheredDocContext,
    best_certain: &mut Option<CandidateMatch>,
    all_candidates: &mut Vec<CandidateMatch>,
) {
    for src in ordered_sources {
        if best_certain.is_some() {
            break;
        }
        let hits = fetch_source_hits(db, clients, config, allow_mrlookup, src, working_ids).await;
        if !hits.is_empty() {
            absorb_hits(hits, ctx, best_certain, all_candidates);
        }
    }
}

/// Checks whether a candidate title represents a complete ("total") positive match against
/// `query_title` (excluding short/generic titles, part-number mismatches, and errata/reviews).
/// Returns `Some(true)` on exact normalized equality and `Some(false)` on a near-exact OCR match.
fn is_total_title_match(query_title: &str, rec: &BibliographicRecord) -> Option<bool> {
    if looks_like_garbage_title(query_title) || rec.title.trim().is_empty() {
        return None;
    }
    if is_erratum_or_review_mismatch(query_title, &rec.title) {
        return None;
    }
    let q_clean = clean_metadata_text(query_title);
    let r_clean = clean_metadata_text(&rec.title);
    let q_norm = normalize_title_words(&q_clean);
    let r_norm = normalize_title_words(&r_clean);
    let q_sig = significant_title_tokens(&q_norm).len();
    if q_sig < 3 || q_norm.len() < 18 {
        return None;
    }
    if q_norm == r_norm {
        return Some(true);
    }

    let (full_sim, _, _) = compute_title_similarity(query_title, &rec.title);
    let pair_sim = pair_title_sim(&q_norm, &r_norm);
    if q_sig < 4 || q_norm.len() < 24 || pair_sim < 0.975 || full_sim < 0.985 {
        return None;
    }
    let q_toks: Vec<&str> = q_norm.split_whitespace().collect();
    let r_toks: Vec<&str> = r_norm.split_whitespace().collect();
    if q_toks.len().abs_diff(r_toks.len()) > 1 {
        return None;
    }
    fn short_toks<'a>(toks: &[&'a str]) -> Vec<&'a str> {
        toks.iter().copied().filter(|t| t.len() < 3).collect()
    }
    if short_toks(&q_toks) != short_toks(&r_toks) {
        return None;
    }
    Some(false)
}

enum TotalTitlePick {
    Match(Box<CandidateMatch>),
    AmbiguousCollision,
    None,
}

fn pick_unambiguous_total_title_match(
    query_title: &str,
    hits: &[CandidateMatch],
    ctx: &GatheredDocContext,
) -> TotalTitlePick {
    let mut verified: Vec<(bool, &CandidateMatch)> = hits
        .iter()
        .filter_map(|cand| {
            let exact = is_total_title_match(query_title, &cand.record)?;
            verify_candidate_against_context(cand, ctx).then_some((exact, cand))
        })
        .collect();
    if verified.is_empty() {
        return TotalTitlePick::None;
    }
    verified.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| {
                record_completeness_score(&b.1.record)
                    .cmp(&record_completeness_score(&a.1.record))
            })
            .then_with(|| b.1.score.total_cmp(&a.1.score))
    });
    let (best_exact, best) = verified[0];
    let has_collision = verified.iter().skip(1).any(|(other_exact, other)| {
        (!best_exact || *other_exact) && !is_same_work(&best.record, &other.record)
    });
    if has_collision {
        TotalTitlePick::AmbiguousCollision
    } else {
        TotalTitlePick::Match(Box::new(best.clone()))
    }
}

fn push_unique_verified_candidates(
    mut hits: Vec<CandidateMatch>,
    ctx: &GatheredDocContext,
    all_candidates: &mut Vec<CandidateMatch>,
) {
    for h in &mut hits {
        if !verify_candidate_against_context(h, ctx) {
            h.score *= 0.35;
        }
        if !all_candidates
            .iter()
            .any(|existing| existing.source == h.source && is_same_work(&existing.record, &h.record))
        {
            all_candidates.push(h.clone());
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_final_title_only_double_check(
    db: &Database,
    clients: &WaterfallClients,
    config: &AppConfig,
    allow_mrlookup: bool,
    ordered_sources: &[String],
    working_ids: &ExtractedIdentifiers,
    ctx: &GatheredDocContext,
    best_certain: &mut Option<CandidateMatch>,
    all_candidates: &mut Vec<CandidateMatch>,
) {
    // If the user already locked non-title fields, respect those constraints rather than dropping them.
    if ctx.is_guided {
        let only_title_locked = ctx.locked_title
            && !ctx.locked_authors
            && !ctx.locked_year
            && working_ids.doi.is_none()
            && working_ids.arxiv_id.is_none()
            && working_ids.mr_number.is_none()
            && working_ids.zbl_number.is_none()
            && working_ids.isbn.is_none();
        if only_title_locked {
            if let Some(ref q_title) = working_ids.heuristic_title {
                if let TotalTitlePick::Match(chosen) =
                    pick_unambiguous_total_title_match(q_title, all_candidates, ctx)
                {
                    *best_certain = Some(*chosen);
                }
            }
        }
        return;
    }

    let mut title_candidates: Vec<String> = Vec::new();
    let mut seen_norms: HashSet<String> = HashSet::new();
    for raw in [&working_ids.heuristic_title, &ctx.db_title]
        .into_iter()
        .flatten()
    {
        let trimmed = raw.trim();
        if looks_like_garbage_title(trimmed) {
            continue;
        }
        let norm = normalize_title_words(&clean_metadata_text(trimmed));
        if significant_title_tokens(&norm).len() < 3 || norm.len() < 18 {
            continue;
        }
        if seen_norms.insert(norm) {
            title_candidates.push(trimmed.to_string());
        }
    }

    let pass1_was_title_only = working_ids.doi.is_none()
        && working_ids.pii.is_none()
        && working_ids.arxiv_id.is_none()
        && working_ids.isbn.is_none()
        && working_ids.mr_number.is_none()
        && working_ids.zbl_number.is_none()
        && working_ids.heuristic_first_author.is_none()
        && working_ids.heuristic_authors.is_empty()
        && working_ids.heuristic_year.is_none()
        && working_ids.filename_keywords.is_empty();

    for q_title in title_candidates {
        if best_certain.is_some() {
            break;
        }
        if pass1_was_title_only
            && working_ids
                .heuristic_title
                .as_deref()
                .map(str::trim)
                == Some(q_title.as_str())
        {
            if let TotalTitlePick::Match(chosen) =
                pick_unambiguous_total_title_match(&q_title, all_candidates, ctx)
            {
                *best_certain = Some(*chosen);
                break;
            }
            continue;
        }

        let title_only_ids = ExtractedIdentifiers {
            heuristic_title: Some(q_title.clone()),
            looks_like_book: working_ids.looks_like_book,
            ..ExtractedIdentifiers::default()
        };
        for src in ordered_sources {
            let hits = fetch_source_hits(
                db,
                clients,
                config,
                allow_mrlookup,
                src,
                &title_only_ids,
            )
            .await;
            if hits.is_empty() {
                continue;
            }
            push_unique_verified_candidates(hits.clone(), ctx, all_candidates);
            match pick_unambiguous_total_title_match(&q_title, &hits, ctx) {
                TotalTitlePick::Match(chosen) => {
                    *best_certain = Some(*chosen);
                    break;
                }
                TotalTitlePick::AmbiguousCollision => break,
                TotalTitlePick::None => {}
            }
        }
    }
}

fn enrich_working_ids_from_bridge(
    working_ids: &mut ExtractedIdentifiers,
    bridge: &BibliographicRecord,
    ctx: &GatheredDocContext,
) {
    if !ctx.locked_title && !bridge.title.trim().is_empty() {
        working_ids.heuristic_title = Some(bridge.title.clone());
    }
    if !ctx.locked_authors && !bridge.authors.is_empty() {
        if let Some(first) = bridge.authors.first() {
            if !first.family_name.trim().is_empty() {
                working_ids.heuristic_first_author = Some(first.family_name.clone());
            }
        }
        working_ids.heuristic_authors = bridge
            .authors
            .iter()
            .filter_map(|a| {
                if a.family_name.trim().is_empty() {
                    None
                } else if a.given_name.trim().is_empty() {
                    Some(a.family_name.clone())
                } else {
                    Some(format!("{}, {}", a.family_name.trim(), a.given_name.trim()))
                }
            })
            .collect();
    }
    if !ctx.locked_year && bridge.year.is_some() {
        working_ids.heuristic_year = bridge.year;
    }
}

/// Fills missing bibliographic/identifier fields on the winning record from corroborated records
/// of the exact same work (e.g. combining MRLookup's MR number + BibTeX with Crossref/zbMATH's DOI, Zbl, abstract).
fn enrich_record_from_same_work(
    target: &mut BibliographicRecord,
    donor: &BibliographicRecord,
) {
    if target.doi.is_none() {
        target.doi = donor.doi.clone();
    }
    if target.arxiv_id.is_none() {
        target.arxiv_id = donor.arxiv_id.clone();
        target.arxiv_version = donor.arxiv_version.clone();
    }
    if target.mr_number.is_none() {
        target.mr_number = donor.mr_number.clone();
        target.mr_number_display = donor.mr_number_display.clone();
    }
    if target.zbl_number.is_none() {
        target.zbl_number = donor.zbl_number.clone();
    }
    if target.msc_primary.is_none() {
        target.msc_primary = donor.msc_primary.clone();
    }
    if target.msc_secondary.is_empty() && !donor.msc_secondary.is_empty() {
        target.msc_secondary = donor.msc_secondary.clone();
    }
    if target.issn.is_none() {
        target.issn = donor.issn.clone();
    }
    if target.isbn.is_none() {
        target.isbn = donor.isbn.clone();
    }
    if target.abstract_text.is_none() {
        target.abstract_text = donor.abstract_text.clone();
    }
    if target.url.is_none() {
        target.url = donor.url.clone();
    }
}

fn absorb_hits(
    mut hits: Vec<CandidateMatch>,
    ctx: &GatheredDocContext,
    best_certain: &mut Option<CandidateMatch>,
    all_candidates: &mut Vec<CandidateMatch>,
) {
    for h in &mut hits {
        if !verify_candidate_against_context(h, ctx) {
            h.score *= 0.35;
        }
    }
    hits.sort_by(|a, b| {
        let diff = (b.score - a.score).abs();
        if diff <= 0.035 && is_same_work(&a.record, &b.record) {
            record_completeness_score(&b.record)
                .cmp(&record_completeness_score(&a.record))
                .then_with(|| b.score.total_cmp(&a.score))
        } else {
            b.score.total_cmp(&a.score)
        }
    });

    if let Some(top) = hits.first() {
        let second_distinct = hits
            .iter()
            .skip(1)
            .find(|cand| !is_same_work(&top.record, &cand.record));
        if is_high_confidence(top, second_distinct) {
            *best_certain = Some(top.clone());
        } else {
            all_candidates.extend(hits);
        }
    }
}

fn is_high_confidence(top: &CandidateMatch, second_distinct: Option<&CandidateMatch>) -> bool {
    if top.score >= 0.98 {
        return true;
    }
    let gap = second_distinct.map(|s| top.score - s.score).unwrap_or(1.0);
    if top.score >= 0.95 {
        return gap >= 0.02;
    }
    if top.score >= 0.90 {
        return gap >= 0.03;
    }
    if top.score >= 0.85 {
        return gap >= 0.08;
    }
    if top.score >= 0.80 {
        return gap >= 0.14;
    }
    false
}

fn record_completeness_score(rec: &BibliographicRecord) -> i32 {
    let mut pts = 0;
    if rec.work_type != WorkType::Misc {
        pts += 3;
    }
    if rec
        .journal_full
        .as_deref()
        .or(rec.journal_abbrev.as_deref())
        .is_some_and(|j| {
            let low = j.to_ascii_lowercase();
            !low.is_empty()
                && low != "corr"
                && !low.contains("arxiv")
                && !low.contains("ssrn")
                && !low.contains("preprint")
        })
    {
        pts += 4;
    }
    if rec.volume.is_some() {
        pts += 2;
    }
    if rec.pages.is_some() {
        pts += 2;
    }
    if rec.doi.as_deref().is_some_and(|d| !d.starts_with("10.48550/")) {
        pts += 2;
    }
    if rec.mr_number.is_some() {
        pts += 3;
    }
    if rec.zbl_number.is_some() {
        pts += 2;
    }
    if rec.msc_primary.is_some() {
        pts += 1;
    }
    pts
}

fn is_same_work(a: &BibliographicRecord, b: &BibliographicRecord) -> bool {
    if let (Some(da), Some(db)) = (a.doi.as_deref(), b.doi.as_deref()) {
        if !da.is_empty() && da == db {
            return true;
        }
    }
    if !titles_represent_same_work(&a.title, &b.title) {
        return false;
    }
    if a.authors.is_empty() || b.authors.is_empty() {
        return true;
    }
    authors_share_surname(&a.authors, &b.authors)
}

fn authors_share_surname(a: &[BibliographicAuthor], b: &[BibliographicAuthor]) -> bool {
    for aa in a {
        let sa = normalize_math_text(&aa.family_name);
        if sa.len() < 2 {
            continue;
        }
        for bb in b {
            let sb = normalize_math_text(&bb.family_name);
            if sb.len() < 2 {
                continue;
            }
            if sa == sb || strsim::jaro_winkler(&sa, &sb) >= 0.88 {
                return true;
            }
        }
    }
    false
}

fn word_stem_without_last_char(w: &str) -> Option<&str> {
    if w.chars().count() < 6 {
        return None;
    }
    let (last_char_idx, _) = w.char_indices().next_back()?;
    w.get(..last_char_idx).filter(|stem| !stem.is_empty())
}

fn verify_candidate_against_context(cand: &CandidateMatch, ctx: &GatheredDocContext) -> bool {
    let rec = &cand.record;

    // 1. In guided mode, never override an author explicitly locked by the user
    if ctx.is_guided && ctx.locked_authors && !ctx.ids.heuristic_authors.is_empty() {
        let locked_surnames: Vec<String> = ctx
            .ids
            .heuristic_authors
            .iter()
            .filter_map(|a| clean_author_surname(a))
            .map(|s| normalize_math_text(&s))
            .filter(|s| s.len() >= 2)
            .collect();
        if !locked_surnames.is_empty() && !rec.authors.is_empty() {
            let matched = locked_surnames.iter().any(|ls| {
                rec.authors.iter().any(|ra| {
                    let rs = normalize_math_text(&ra.family_name);
                    !rs.is_empty()
                        && (rs == *ls
                            || rs.contains(ls.as_str())
                            || ls.contains(rs.as_str())
                            || strsim::jaro_winkler(&rs, ls) >= 0.86)
                })
            });
            if !matched {
                return false;
            }
        }
        return true;
    }

    if ctx.is_guided {
        return true;
    }

    // 2. Zotero-style PDF front-matter cross-check in automatic mode
    let Some(ref vtxt) = ctx.verify_text else {
        return true;
    };
    let v_words: Vec<&str> = vtxt.split_whitespace().collect();
    if v_words.len() < 12 {
        return true;
    };

    let author_in_pdf = if rec.authors.is_empty() {
        false
    } else {
        rec.authors.iter().any(|au| {
            let fam_norm = normalize_math_text(&au.family_name);
            let tokens: Vec<&str> = fam_norm.split_whitespace().collect();
            let has_long_tok = tokens.iter().any(|tok| tok.chars().count() >= 3);
            tokens.into_iter().any(|tok| {
                let len = tok.chars().count();
                if len >= 3 {
                    vtxt.contains(tok)
                        || v_words
                            .iter()
                            .any(|vw| vw.chars().count() >= 4 && strsim::jaro_winkler(vw, tok) >= 0.91)
                } else if !has_long_tok && len == 2 {
                    v_words.contains(&tok)
                } else {
                    false
                }
            })
        })
    };

    let sig_words = significant_title_words_vec(&rec.title);
    let matched_title_words = sig_words
        .iter()
        .filter(|w| {
            vtxt.contains(w.as_str())
                || word_stem_without_last_char(w).is_some_and(|stem| vtxt.contains(stem))
                || v_words
                    .iter()
                    .any(|vw| vw.chars().count() >= 5 && strsim::jaro_winkler(vw, w) >= 0.91)
        })
        .count();
    let title_ratio = if sig_words.is_empty() {
        1.0
    } else {
        (matched_title_words as f32) / (sig_words.len() as f32)
    };

    // Exact identifier match (DOI / arXiv / MR / Zbl): guard against footnote-citation DOIs on page 1
    if cand.score >= 0.985 {
        if vtxt.len() >= 140 {
            return author_in_pdf || matched_title_words >= 2 || title_ratio >= 0.45;
        }
        return true;
    }

    // Heuristic Title / Author match: candidate's author AND core title words must appear in pages 1-2
    if !rec.authors.is_empty() && !author_in_pdf {
        return false;
    }
    if !sig_words.is_empty() && title_ratio < 0.45 && matched_title_words < 4 {
        return false;
    }
    true
}

struct DocIdRow {
    title: String,
    first_author: String,
    year: Option<i64>,
    doi: Option<String>,
    arxiv_id: Option<String>,
    arxiv_ver: Option<String>,
    mr_num: Option<String>,
    zbl_num: Option<String>,
    isbn: Option<String>,
    work_type: String,
    metadata_source: Option<String>,
    file_format: Option<String>,
    root_path: Option<String>,
    rel_path: Option<String>,
    file_available: bool,
    page_count: u32,
}

struct GatheredDocContext {
    ids: ExtractedIdentifiers,
    db_title: Option<String>,
    verify_text: Option<String>,
    is_guided: bool,
    locked_title: bool,
    locked_authors: bool,
    locked_year: bool,
}

#[cfg(test)]
fn gather_document_identifiers(
    db: &Database,
    config: &AppConfig,
    document_id: i64,
) -> Result<ExtractedIdentifiers> {
    Ok(gather_document_context(db, config, document_id)?.ids)
}

fn gather_document_context(
    db: &Database,
    config: &AppConfig,
    document_id: i64,
) -> Result<GatheredDocContext> {
    let conn = db.reader();
    let locked = load_locked_fields(&conn, document_id)?;
    let DocIdRow {
        title,
        first_author,
        year,
        doi,
        arxiv_id,
        arxiv_ver,
        mr_num,
        zbl_num,
        isbn,
        work_type,
        metadata_source,
        file_format,
        root_path,
        rel_path,
        file_available,
        page_count,
    } = conn.query_row(
        "SELECT d.title, d.authors_norm_summary, d.year, d.doi, d.arxiv_id, d.arxiv_version,
                d.mr_number, d.zbl_number, d.isbn, d.work_type, d.metadata_source,
                (SELECT f.format FROM files f WHERE f.document_id = d.id ORDER BY f.is_available DESC, f.id ASC LIMIT 1),
                (SELECT sr.root_path FROM files f JOIN source_roots sr ON sr.id = f.root_id WHERE f.document_id = d.id ORDER BY f.is_available DESC, f.id ASC LIMIT 1),
                (SELECT f.rel_path FROM files f WHERE f.document_id = d.id ORDER BY f.is_available DESC, f.id ASC LIMIT 1),
                COALESCE((SELECT f.is_available FROM files f WHERE f.document_id = d.id ORDER BY f.is_available DESC, f.id ASC LIMIT 1), 0),
                COALESCE((SELECT MAX(f.page_count) FROM files f WHERE f.document_id = d.id), 0)
         FROM documents d WHERE d.id = ?1",
        params![document_id],
        |r| {
            Ok(DocIdRow {
                title: r.get(0)?,
                first_author: r.get(1)?,
                year: r.get(2)?,
                doi: r.get(3)?,
                arxiv_id: r.get(4)?,
                arxiv_ver: r.get(5)?,
                mr_num: r.get(6)?,
                zbl_num: r.get(7)?,
                isbn: r.get(8)?,
                work_type: r.get(9)?,
                metadata_source: r.get(10)?,
                file_format: r.get(11)?,
                root_path: r.get(12)?,
                rel_path: r.get(13)?,
                file_available: r.get::<_, i64>(14)? != 0,
                page_count: r.get::<_, i64>(15)? as u32,
            })
        },
    )?;

    let mut db_authors: Vec<String> = {
        let mut a_stmt = conn.prepare(
            "SELECT family_name, given_name
             FROM document_authors
             WHERE document_id = ?1 AND role = 'author'
             ORDER BY position ASC",
        )?;
        let rows = a_stmt.query_map(params![document_id], |r| {
            let fam: String = r.get(0)?;
            let giv: String = r.get(1)?;
            let fam_t = fam.trim();
            let giv_t = giv.trim();
            if fam_t.is_empty() && giv_t.is_empty() {
                Ok(String::new())
            } else if giv_t.is_empty() {
                Ok(fam_t.to_string())
            } else if fam_t.is_empty() {
                Ok(giv_t.to_string())
            } else {
                Ok(format!("{fam_t}, {giv_t}"))
            }
        })?;
        rows.filter_map(std::result::Result::ok)
            .filter(|s| !s.is_empty())
            .collect()
    };
    if db_authors.is_empty() && !first_author.trim().is_empty() {
        if let Some(first_part) = first_author.split(';').next().map(str::trim).filter(|s| !s.is_empty()) {
            db_authors.push(first_part.to_string());
        }
    }

    let stripped_title = strip_inline_markup_tags(&title);
    let db_title = (!stripped_title.trim().is_empty() && !looks_like_garbage_title(&stripped_title))
        .then_some(stripped_title.clone());
    let locked_title_flag = locked.contains("title");
    let locked_authors_flag = locked.contains("authors");
    let locked_year_flag = locked.contains("year");
    let has_locked_anchor = locked.iter().any(|f| {
        matches!(
            f.as_str(),
            "title" | "authors" | "doi" | "arxiv_id" | "mr_number" | "zbl_number" | "isbn"
        )
    });

    // Guided lookup mode: when the user explicitly locked any identifying field (e.g. Title and/or Authors),
    // search strictly using the user's locked fields and ignore noisy PDF headers/filenames/unlocked stale IDs.
    if has_locked_anchor {
        let locked_authors = if locked_authors_flag {
            db_authors
        } else {
            Vec::new()
        };
        let locked_first_author = locked_authors.first().cloned();
        let locked_title = if locked_title_flag && !stripped_title.trim().is_empty() {
            Some(stripped_title)
        } else {
            None
        };
        let looks_like_book = if locked.contains("work_type") {
            work_type == "book"
        } else {
            work_type == "book" || page_count >= 90
        };
        let guided_alt_titles = locked_title
            .as_deref()
            .and_then(extract_prose_core_title)
            .into_iter()
            .collect();
        return Ok(GatheredDocContext {
            ids: ExtractedIdentifiers {
                doi: if locked.contains("doi") {
                    doi.as_deref().and_then(normalize_doi)
                } else {
                    None
                },
                pii: None,
                arxiv_id: if locked.contains("arxiv_id") {
                    arxiv_id
                } else {
                    None
                },
                arxiv_version: if locked.contains("arxiv_id") {
                    arxiv_ver
                } else {
                    None
                },
                isbn: if locked.contains("isbn") { isbn } else { None },
                mr_number: if locked.contains("mr_number") {
                    mr_num.as_deref().and_then(normalize_mr_digits)
                } else {
                    None
                },
                zbl_number: if locked.contains("zbl_number") {
                    zbl_num
                } else {
                    None
                },
                heuristic_title: locked_title,
                alt_heuristic_titles: guided_alt_titles,
                heuristic_first_author: locked_first_author,
                heuristic_authors: locked_authors,
                heuristic_year: if locked_year_flag { year } else { None },
                filename_keywords: Vec::new(),
                looks_like_book,
            },
            db_title,
            verify_text: None,
            is_guided: true,
            locked_title: locked_title_flag,
            locked_authors: locked_authors_flag,
            locked_year: locked_year_flag,
        });
    }

    let mut stmt = conn.prepare(
        "SELECT page_ord, text_content FROM passages
         WHERE document_id = ?1 AND (page_ord <= 4 OR page_ord = (SELECT MAX(page_ord) FROM passages WHERE document_id = ?1))
         ORDER BY page_ord ASC, chunk_index ASC",
    )?;
    let rows = stmt.query_map(params![document_id], |r| {
        Ok((r.get::<_, i64>(0)? as u32, r.get::<_, String>(1)?))
    })?;

    let mut db_first_pages: Vec<String> = Vec::new();
    let mut db_page123_chunks: Vec<String> = Vec::new();
    let mut db_last_page: Option<String> = None;
    for (ord, txt) in rows.filter_map(std::result::Result::ok) {
        if ord <= 4 {
            if ord <= 3 {
                db_page123_chunks.push(txt.clone());
            }
            db_first_pages.push(txt);
        } else {
            db_last_page = Some(txt);
        }
    }
    drop(stmt);
    drop(conn);

    // Inspect primary file front matter (embedded PDF Info / XMP + pages 1-2 layout bounding boxes)
    let front_matter = if file_available {
        match (file_format.as_deref(), root_path.as_deref(), rel_path.as_deref()) {
            (Some(fmt), Some(rp), Some(rel)) => {
                resolve_rel_path(std::path::Path::new(rp), rel)
                    .ok()
                    .filter(|p| p.exists())
                    .and_then(|abs| match fmt {
                        "pdf" => extract_pdf_front_matter(&abs, config.pdfium_lib_path.as_deref()).ok(),
                        "djvu" => extract_djvu_front_matter(&abs, config.djvused_bin.as_deref()).ok(),
                        _ => None,
                    })
            }
            _ => None,
        }
    } else {
        None
    };

    let fm_has_text = front_matter.as_ref().is_some_and(|fm| {
        fm.has_text_layer && fm.pages.iter().any(|p| !p.raw_text.trim().is_empty())
    });

    let fm_first_pages: Vec<&str> = if fm_has_text {
        front_matter
            .as_ref()
            .map(|fm| {
                fm.pages
                    .iter()
                    .filter(|p| p.page_ord <= 4)
                    .map(|p| p.raw_text.as_str())
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let fm_last_page: Option<&str> = if fm_has_text {
        front_matter.as_ref().and_then(|fm| {
            fm.pages
                .iter()
                .find(|p| p.page_ord > 4)
                .map(|p| p.raw_text.as_str())
        })
    } else {
        None
    };

    let db_first_refs: Vec<&str> = db_first_pages.iter().map(String::as_str).collect();
    let effective_first_pages: &[&str] = if !fm_first_pages.is_empty() {
        &fm_first_pages
    } else {
        &db_first_refs
    };
    let effective_last_page = fm_last_page.or(db_last_page.as_deref());
    let effective_page_count = front_matter
        .as_ref()
        .map(|fm| fm.page_count.max(page_count))
        .unwrap_or(page_count);
    let page1_layout = front_matter
        .as_ref()
        .and_then(|fm| fm.pages.iter().find(|p| p.page_ord == 1 && !p.spans.is_empty()));
    let page2_layout = front_matter
        .as_ref()
        .and_then(|fm| fm.pages.iter().find(|p| p.page_ord == 2 && !p.spans.is_empty()));

    let is_heuristic = metadata_source.as_deref().unwrap_or("heuristic") == "heuristic";

    // Only pass DB title/author as authoritative when confirmed/locked; otherwise use actual PDF/DjVu header fields
    let doc_title_hint = if (!is_heuristic || locked_title_flag)
        && !stripped_title.trim().is_empty()
        && !looks_like_garbage_title(&stripped_title)
    {
        Some(stripped_title.as_str())
    } else {
        front_matter
            .as_ref()
            .and_then(|fm| fm.doc_title.as_deref())
    };

    let structured_first_author = db_authors.first().map(String::as_str).unwrap_or("");
    let doc_author_hint = if (!is_heuristic || locked_authors_flag)
        && !structured_first_author.is_empty()
    {
        Some(structured_first_author)
    } else {
        front_matter
            .as_ref()
            .and_then(|fm| fm.doc_author.as_deref())
    };

    let filename_stem = rel_path
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_stem())
        .and_then(|s| s.to_str());

    let mut extracted = extract_identifiers_and_heuristics_with_front_pages(
        front_matter.as_ref().and_then(|fm| fm.xmp_xml.as_deref()),
        doc_title_hint,
        doc_author_hint,
        effective_first_pages,
        effective_last_page,
        effective_page_count,
        filename_stem,
        page1_layout,
        page2_layout,
    );

    // Last-resort fallback for synthetic/fileless rows where only `documents.title` was populated
    if extracted.heuristic_title.is_none()
        && filename_stem.is_none()
        && effective_first_pages.is_empty()
        && !stripped_title.trim().is_empty()
        && !looks_like_garbage_title(&stripped_title)
    {
        extracted.heuristic_title = Some(stripped_title.clone());
    }

    let has_extracted_pages = !effective_first_pages.is_empty() || fm_has_text;

    // Preserve DB identifiers when already confirmed/manual OR when row is synthetic/textless;
    // when re-scanning a heuristic PDF with text pages, trust fresh page-1/2 extraction so old
    // broken/bibliography DOIs get healed instead of sticking forever.
    if !is_heuristic || locked.contains("doi") || !has_extracted_pages {
        if let Some(norm_d) = doi.as_deref().and_then(normalize_doi) {
            extracted.doi = Some(norm_d);
        }
    }
    if (!is_heuristic || locked.contains("arxiv_id") || !has_extracted_pages) && arxiv_id.is_some() {
        extracted.arxiv_id = arxiv_id;
        extracted.arxiv_version = arxiv_ver;
    }
    if !is_heuristic || locked.contains("mr_number") || !has_extracted_pages {
        if let Some(norm_mr) = mr_num.as_deref().and_then(normalize_mr_digits) {
            extracted.mr_number = Some(norm_mr);
        }
    }
    if (!is_heuristic || locked.contains("zbl_number") || !has_extracted_pages) && zbl_num.is_some()
    {
        extracted.zbl_number = zbl_num;
    }
    if (!is_heuristic || locked.contains("isbn") || !has_extracted_pages) && isbn.is_some() {
        extracted.isbn = isbn;
    }
    if ((!is_heuristic || locked_year_flag) && year.is_some())
        || (extracted.heuristic_year.is_none()
            && filename_stem.is_none()
            && effective_first_pages.is_empty())
    {
        extracted.heuristic_year = year;
    }
    if work_type == "book" {
        extracted.looks_like_book = true;
    }

    // Build normalized pages 1-3 verification text (covering repository cover page + article pages 1-2)
    let mut verify_raw = String::new();
    if !fm_first_pages.is_empty() {
        for p in fm_first_pages.iter().take(3) {
            verify_raw.push_str(p);
            verify_raw.push('\n');
        }
    } else {
        for p in db_page123_chunks.iter() {
            verify_raw.push_str(p);
            verify_raw.push('\n');
        }
    }
    if let Some(stem) = filename_stem {
        verify_raw.push(' ');
        verify_raw.push_str(stem);
    }
    let verify_norm = normalize_math_text(&verify_raw);
    let alpha_count = verify_norm
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .count();
    let verify_text = (alpha_count >= 60).then_some(verify_norm);

    Ok(GatheredDocContext {
        ids: extracted,
        db_title,
        verify_text,
        is_guided: false,
        locked_title: locked_title_flag,
        locked_authors: locked_authors_flag,
        locked_year: locked_year_flag,
    })
}

fn load_locked_fields(conn: &Connection, document_id: i64) -> Result<HashSet<String>> {
    let raw: String = conn
        .query_row(
            "SELECT manual_locked_fields FROM documents WHERE id = ?1",
            params![document_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_else(|| "[]".to_string());
    let vec: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
    Ok(vec.into_iter().collect())
}

/// Applies a fetched `BibliographicRecord` to `documents` while NEVER overwriting any field
/// listed in `manual_locked_fields` or any document whose metadata has been confirmed (`metadata_verified != 0`)
/// (unless `ignore_locks` is true).
pub fn apply_record_respecting_locks(
    conn: &Connection,
    document_id: i64,
    rec: &BibliographicRecord,
    ignore_locks: bool,
) -> Result<()> {
    if !ignore_locks && is_document_metadata_verified(conn, document_id)? {
        return Ok(());
    }
    let locked = if ignore_locks {
        HashSet::new()
    } else {
        load_locked_fields(conn, document_id)?
    };
    let now = chrono::Utc::now().timestamp();

    let tx = conn.unchecked_transaction()?;

    let has_bibtex_field_lock = !ignore_locks
        && locked.iter().any(|f| {
            matches!(
                f.as_str(),
                "work_type"
                    | "title"
                    | "authors"
                    | "year"
                    | "journal_abbrev"
                    | "journal_full"
                    | "booktitle"
                    | "series"
                    | "volume"
                    | "issue_number"
                    | "pages"
                    | "edition"
                    | "publisher"
                    | "address"
                    | "isbn"
                    | "issn"
                    | "doi"
                    | "mr_number"
                    | "mr_reviewer"
                    | "msc_primary"
                    | "msc_secondary"
                    | "url"
            )
        });
    let effective_raw_bibtex = if has_bibtex_field_lock {
        None
    } else {
        rec.raw_mrlookup_bibtex.as_deref()
    };

    // Update scalar fields respecting per-field locks; unlocked fields reflect the newly matched record
    // directly so stale values from a previously misidentified paper never persist.
    let title_norm = normalize_math_text(&rec.title);
    let pages_norm = rec.pages.as_deref().map(normalize_pages);
    let doi_norm = rec.doi.as_deref().and_then(normalize_doi);
    let mr_norm = rec.mr_number.as_deref().and_then(normalize_mr_digits);

    tx.execute(
        "UPDATE documents SET
            work_type = CASE WHEN ?2 = 0 THEN ?3 ELSE work_type END,
            title = CASE WHEN ?4 = 0 AND ?5 != '' THEN ?5 ELSE title END,
            title_norm = CASE WHEN ?4 = 0 AND ?5 != '' THEN ?6 ELSE title_norm END,
            journal_abbrev = CASE WHEN ?7 = 0 THEN ?8 ELSE journal_abbrev END,
            journal_full = CASE WHEN ?9 = 0 THEN ?10 ELSE journal_full END,
            booktitle = CASE WHEN ?11 = 0 THEN ?12 ELSE booktitle END,
            series = CASE WHEN ?13 = 0 THEN ?14 ELSE series END,
            volume = CASE WHEN ?15 = 0 THEN ?16 ELSE volume END,
            issue_number = CASE WHEN ?17 = 0 THEN ?18 ELSE issue_number END,
            year = CASE WHEN ?19 = 0 THEN COALESCE(?20, CASE WHEN COALESCE(metadata_source, 'heuristic') = 'heuristic' THEN year ELSE NULL END) ELSE year END,
            pages = CASE WHEN ?21 = 0 THEN ?22 ELSE pages END,
            edition = CASE WHEN ?23 = 0 THEN ?24 ELSE edition END,
            publisher = CASE WHEN ?25 = 0 THEN ?26 ELSE publisher END,
            address = CASE WHEN ?27 = 0 THEN ?28 ELSE address END,
            isbn = CASE WHEN ?29 = 0 THEN COALESCE(?30, CASE WHEN COALESCE(metadata_source, 'heuristic') = 'heuristic' THEN isbn ELSE NULL END) ELSE isbn END,
            issn = CASE WHEN ?31 = 0 THEN ?32 ELSE issn END,
            doi = CASE WHEN ?33 = 0 THEN COALESCE(?34, CASE WHEN COALESCE(metadata_source, 'heuristic') = 'heuristic' THEN doi ELSE NULL END) ELSE doi END,
            arxiv_id = CASE WHEN ?35 = 0 THEN COALESCE(?36, CASE WHEN COALESCE(metadata_source, 'heuristic') = 'heuristic' THEN arxiv_id ELSE NULL END) ELSE arxiv_id END,
            arxiv_version = CASE WHEN ?35 = 0 THEN COALESCE(?37, CASE WHEN COALESCE(metadata_source, 'heuristic') = 'heuristic' THEN arxiv_version ELSE NULL END) ELSE arxiv_version END,
            mr_number = CASE WHEN ?38 = 0 THEN ?39 ELSE mr_number END,
            mr_number_display = CASE WHEN ?38 = 0 THEN ?40 ELSE mr_number_display END,
            mr_reviewer = CASE WHEN ?41 = 0 THEN ?42 ELSE mr_reviewer END,
            zbl_number = CASE WHEN ?43 = 0 THEN ?44 ELSE zbl_number END,
            url = CASE WHEN ?45 = 0 THEN ?46 ELSE url END,
            abstract_text = CASE WHEN ?47 = 0 THEN ?48 ELSE abstract_text END,
            raw_mrlookup_bibtex = ?49,
            metadata_source = ?50,
            metadata_license_note = ?51,
            updated_at = ?52
         WHERE id = ?1",
        params![
            document_id,
            locked.contains("work_type") as i64,
            rec.work_type.as_str(),
            locked.contains("title") as i64,
            rec.title,
            title_norm,
            locked.contains("journal_abbrev") as i64,
            rec.journal_abbrev,
            locked.contains("journal_full") as i64,
            rec.journal_full,
            locked.contains("booktitle") as i64,
            rec.booktitle,
            locked.contains("series") as i64,
            rec.series,
            locked.contains("volume") as i64,
            rec.volume,
            locked.contains("issue_number") as i64,
            rec.issue_number,
            locked.contains("year") as i64,
            rec.year,
            locked.contains("pages") as i64,
            pages_norm,
            locked.contains("edition") as i64,
            rec.edition,
            locked.contains("publisher") as i64,
            rec.publisher,
            locked.contains("address") as i64,
            rec.address,
            locked.contains("isbn") as i64,
            rec.isbn,
            locked.contains("issn") as i64,
            rec.issn,
            locked.contains("doi") as i64,
            doi_norm,
            locked.contains("arxiv_id") as i64,
            rec.arxiv_id,
            rec.arxiv_version,
            locked.contains("mr_number") as i64,
            mr_norm,
            rec.mr_number_display,
            locked.contains("mr_reviewer") as i64,
            rec.mr_reviewer,
            locked.contains("zbl_number") as i64,
            rec.zbl_number,
            locked.contains("url") as i64,
            rec.url,
            locked.contains("abstract_text") as i64,
            rec.abstract_text,
            effective_raw_bibtex,
            rec.source,
            rec.license_note,
            now
        ],
    )?;

    if !locked.contains("authors") && !rec.authors.is_empty() {
        replace_authors_in_tx(&tx, document_id, &rec.authors)?;
    }

    if !locked.contains("msc_primary") {
        let cur_status: String = tx
            .query_row(
                "SELECT msc_primary_status FROM documents WHERE id = ?1",
                params![document_id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "none".to_string());
        if cur_status != "confirmed" {
            if let Some(msc) = rec.msc_primary.as_deref() {
                tx.execute(
                    "UPDATE documents SET msc_primary = ?1, msc_primary_status = 'metadata' WHERE id = ?2",
                    params![msc, document_id],
                )?;
            } else {
                tx.execute(
                    "UPDATE documents SET msc_primary = NULL, msc_primary_status = 'none' WHERE id = ?1",
                    params![document_id],
                )?;
            }
        }
    }

    if !locked.contains("msc_secondary") {
        tx.execute(
            "DELETE FROM document_msc_secondary WHERE document_id = ?1 AND status != 'confirmed'",
            params![document_id],
        )?;
        for code in &rec.msc_secondary {
            tx.execute(
                "INSERT OR IGNORE INTO document_msc_secondary (document_id, msc_code, status)
                 VALUES (?1, ?2, 'metadata')",
                params![document_id, code],
            )?;
        }
    }

    if !locked.is_empty() {
        let hashes: Vec<String> = {
            let mut st = tx.prepare("SELECT DISTINCT blake3_hash FROM files WHERE document_id = ?1")?;
            let iter = st.query_map(params![document_id], |r| r.get(0))?;
            iter.filter_map(std::result::Result::ok).collect()
        };
        for h in hashes {
            archive_if_manual(&tx, document_id, &h)?;
        }
    }

    tx.commit()?;
    Ok(())
}

fn replace_authors_in_tx(
    tx: &Connection,
    document_id: i64,
    authors: &[BibliographicAuthor],
) -> Result<()> {
    tx.execute(
        "DELETE FROM document_authors WHERE document_id = ?1",
        params![document_id],
    )?;
    let mut role_positions: HashMap<&str, i64> = HashMap::new();
    let mut summary_parts = Vec::new();
    for a in authors {
        let pos = role_positions.entry(a.role.as_str()).or_insert(0);
        let full = if a.given_name.trim().is_empty() {
            a.family_name.clone()
        } else {
            format!("{} {}", a.given_name.trim(), a.family_name.trim())
        };
        let norm = normalize_math_text(&full);
        if a.role == "author" && !norm.is_empty() {
            summary_parts.push(norm.clone());
        }
        tx.execute(
            "INSERT INTO document_authors (document_id, position, family_name, given_name, name_norm, role)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![document_id, *pos, a.family_name, a.given_name, norm, a.role],
        )?;
        *pos += 1;
    }
    let summary = summary_parts.join("; ");
    tx.execute(
        "UPDATE documents SET authors_norm_summary = ?1 WHERE id = ?2",
        params![summary, document_id],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualMetadataEdit {
    pub document_id: i64,
    pub work_type: String,
    pub title: String,
    pub authors: Vec<BibliographicAuthor>,
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
    pub mr_number: Option<String>,
    pub mr_reviewer: Option<String>,
    pub zbl_number: Option<String>,
    pub url: Option<String>,
    pub msc_primary: Option<String>,
    pub msc_secondary: Vec<String>,
    pub abstract_text: Option<String>,
    pub exclude_from_cloud_llm: bool,
    pub tags: Vec<String>,
    /// Explicit list of fields edited in this save so only touched fields are locked.
    pub edited_fields: Vec<String>,
}

/// Applies manual user edits, locks modified fields in `manual_locked_fields`,
/// updates tags, and snapshots into `manual_metadata_archive`.
pub fn apply_manual_metadata_edit(db: &Database, edit: &ManualMetadataEdit) -> Result<DocumentMetadataView> {
    apply_manual_metadata_edit_with_locks(db, edit, None, false)
}

/// Applies manual user edits with optional explicit lock list (`explicit_locks`) and optional
/// clearing of unlocked bibliographic fields (`reset_unlocked`, used before guided re-lookup).
pub fn apply_manual_metadata_edit_with_locks(
    db: &Database,
    edit: &ManualMetadataEdit,
    explicit_locks: Option<&[String]>,
    reset_unlocked: bool,
) -> Result<DocumentMetadataView> {
    {
        let conn = db.writer();
        let now = chrono::Utc::now().timestamp();
        let mut locked: HashSet<String> = match explicit_locks {
            Some(list) => list
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            None => {
                let mut prev = load_locked_fields(&conn, edit.document_id)?;
                for f in &edit.edited_fields {
                    let trimmed = f.trim();
                    if !trimmed.is_empty() {
                        prev.insert(trimmed.to_string());
                    }
                }
                prev
            }
        };
        // Tags and cloud-exclusion are user preferences rather than bibliographic fields
        locked.remove("tags");
        locked.remove("exclude_from_cloud_llm");

        let mut locked_vec: Vec<String> = locked.iter().cloned().collect();
        locked_vec.sort();
        let locked_json = serde_json::to_string(&locked_vec)?;

        let keep = |field: &str| !reset_unlocked || locked.contains(field);

        let effective_work_type = if keep("work_type") {
            WorkType::from_str_lossy(&edit.work_type).as_str().to_string()
        } else {
            "article".to_string()
        };

        let effective_title = if keep("title") {
            let trimmed = edit.title.trim();
            if trimmed.is_empty() {
                edit.title.clone()
            } else {
                trimmed.to_string()
            }
        } else {
            let rel_opt: Option<String> = conn
                .query_row(
                    "SELECT rel_path FROM files WHERE document_id = ?1 ORDER BY is_available DESC, id ASC LIMIT 1",
                    params![edit.document_id],
                    |r| r.get(0),
                )
                .optional()?;
            let stem_fallback = rel_opt
                .as_deref()
                .and_then(|rel| std::path::Path::new(rel).file_stem())
                .and_then(|s| s.to_str())
                .map(|s| s.replace(['_', '-'], " ").trim().to_string())
                .filter(|s| !s.is_empty());
            stem_fallback.unwrap_or_else(|| {
                let t = edit.title.trim();
                if t.is_empty() {
                    "Untitled".to_string()
                } else {
                    t.to_string()
                }
            })
        };

        let title_norm = normalize_math_text(&effective_title);
        let journal_abbrev_val = if keep("journal_abbrev") { clean_opt(&edit.journal_abbrev) } else { None };
        let journal_full_val = if keep("journal_full") { clean_opt(&edit.journal_full) } else { None };
        let booktitle_val = if keep("booktitle") { clean_opt(&edit.booktitle) } else { None };
        let series_val = if keep("series") { clean_opt(&edit.series) } else { None };
        let volume_val = if keep("volume") { clean_opt(&edit.volume) } else { None };
        let issue_val = if keep("issue_number") { clean_opt(&edit.issue_number) } else { None };
        let year_val = if keep("year") { edit.year } else { None };
        let pages_norm = if keep("pages") { edit.pages.as_deref().map(normalize_pages) } else { None };
        let edition_val = if keep("edition") { clean_opt(&edit.edition) } else { None };
        let publisher_val = if keep("publisher") { clean_opt(&edit.publisher) } else { None };
        let address_val = if keep("address") { clean_opt(&edit.address) } else { None };
        let isbn_val = if keep("isbn") { clean_opt(&edit.isbn) } else { None };
        let issn_val = if keep("issn") { clean_opt(&edit.issn) } else { None };
        let doi_norm = if keep("doi") { edit.doi.as_deref().and_then(normalize_doi) } else { None };
        let arxiv_val = if keep("arxiv_id") { clean_opt(&edit.arxiv_id) } else { None };
        let mr_norm = if keep("mr_number") { edit.mr_number.as_deref().and_then(normalize_mr_digits) } else { None };
        let mr_reviewer_val = if keep("mr_reviewer") { clean_opt(&edit.mr_reviewer) } else { None };
        let zbl_val = if keep("zbl_number") { clean_opt(&edit.zbl_number) } else { None };
        let url_val = if keep("url") { clean_opt(&edit.url) } else { None };
        let msc_primary_val = if keep("msc_primary") { clean_opt(&edit.msc_primary) } else { None };
        let abstract_val = if keep("abstract_text") { clean_opt(&edit.abstract_text) } else { None };
        let clear_raw_bib = reset_unlocked || !edit.edited_fields.is_empty() || !locked_vec.is_empty();
        let next_source = if reset_unlocked && locked_vec.is_empty() {
            "heuristic"
        } else {
            "manual"
        };

        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE documents SET
                work_type = ?2,
                title = ?3,
                title_norm = ?4,
                journal_abbrev = ?5,
                journal_full = ?6,
                booktitle = ?7,
                series = ?8,
                volume = ?9,
                issue_number = ?10,
                year = ?11,
                pages = ?12,
                edition = ?13,
                publisher = ?14,
                address = ?15,
                isbn = ?16,
                issn = ?17,
                doi = ?18,
                arxiv_id = ?19,
                arxiv_version = CASE WHEN ?29 != 0 THEN NULL ELSE arxiv_version END,
                mr_number = ?20,
                mr_number_display = CASE WHEN ?30 != 0 THEN NULL ELSE mr_number_display END,
                mr_reviewer = ?21,
                zbl_number = ?22,
                url = ?23,
                msc_primary = ?24,
                msc_primary_status = CASE WHEN ?24 IS NOT NULL AND ?24 != '' THEN 'confirmed' ELSE 'none' END,
                abstract_text = ?25,
                raw_mrlookup_bibtex = CASE WHEN ?31 != 0 THEN NULL ELSE raw_mrlookup_bibtex END,
                metadata_license_note = CASE WHEN ?32 != 0 THEN NULL ELSE metadata_license_note END,
                manual_locked_fields = ?26,
                exclude_from_cloud_llm = ?27,
                metadata_source = ?33,
                metadata_verified = CASE WHEN ?32 != 0 THEN 0 ELSE metadata_verified END,
                updated_at = ?28
             WHERE id = ?1",
            params![
                edit.document_id,
                effective_work_type,
                effective_title,
                title_norm,
                journal_abbrev_val,
                journal_full_val,
                booktitle_val,
                series_val,
                volume_val,
                issue_val,
                year_val,
                pages_norm,
                edition_val,
                publisher_val,
                address_val,
                isbn_val,
                issn_val,
                doi_norm,
                arxiv_val,
                mr_norm,
                mr_reviewer_val,
                zbl_val,
                url_val,
                msc_primary_val,
                abstract_val,
                locked_json,
                edit.exclude_from_cloud_llm as i64,
                now,
                (reset_unlocked && !locked.contains("arxiv_id")) as i64,
                (reset_unlocked && !locked.contains("mr_number")) as i64,
                clear_raw_bib as i64,
                reset_unlocked as i64,
                next_source,
            ],
        )?;

        let effective_authors: &[BibliographicAuthor] = if keep("authors") {
            &edit.authors
        } else {
            &[]
        };
        replace_authors_in_tx(&tx, edit.document_id, effective_authors)?;

        tx.execute(
            "DELETE FROM document_msc_secondary WHERE document_id = ?1",
            params![edit.document_id],
        )?;
        if keep("msc_secondary") {
            for code in &edit.msc_secondary {
                let trimmed = code.trim();
                if !trimmed.is_empty() {
                    tx.execute(
                        "INSERT OR IGNORE INTO document_msc_secondary (document_id, msc_code, status)
                         VALUES (?1, ?2, 'confirmed')",
                        params![edit.document_id, trimmed],
                    )?;
                }
            }
        }

        if reset_unlocked {
            tx.execute(
                "DELETE FROM metadata_candidates WHERE document_id = ?1",
                params![edit.document_id],
            )?;
        }

        // Update tags
        tx.execute(
            "DELETE FROM document_tags WHERE document_id = ?1",
            params![edit.document_id],
        )?;
        for tag in &edit.tags {
            let t = tag.trim();
            if t.is_empty() {
                continue;
            }
            tx.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", params![t])?;
            let tag_id: i64 = tx.query_row(
                "SELECT id FROM tags WHERE name = ?1",
                params![t],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO document_tags (document_id, tag_id) VALUES (?1, ?2)",
                params![edit.document_id, tag_id],
            )?;
        }

        // Snapshot into manual_metadata_archive for all files of this document
        let hashes: Vec<String> = {
            let mut st = tx.prepare("SELECT DISTINCT blake3_hash FROM files WHERE document_id = ?1")?;
            let iter = st.query_map(params![edit.document_id], |r| r.get(0))?;
            iter.filter_map(std::result::Result::ok).collect()
        };
        for h in hashes {
            archive_if_manual(&tx, edit.document_id, &h)?;
        }

        tx.commit()?;
    }

    get_document_metadata(db, edit.document_id)
}

pub fn confirm_metadata_candidate(
    db: &Database,
    document_id: i64,
    candidate_id: i64,
) -> Result<DocumentMetadataView> {
    {
        let conn = db.writer();
        let json: String = conn.query_row(
            "SELECT candidate_json FROM metadata_candidates WHERE id = ?1 AND document_id = ?2",
            params![candidate_id, document_id],
            |r| r.get(0),
        )?;
        let rec: BibliographicRecord = serde_json::from_str(&json)?;
        apply_record_respecting_locks(&conn, document_id, &rec, false)?;
        conn.execute(
            "DELETE FROM metadata_candidates WHERE document_id = ?1",
            params![document_id],
        )?;
    }
    get_document_metadata(db, document_id)
}

pub fn dismiss_metadata_candidate(db: &Database, candidate_id: i64) -> Result<()> {
    let conn = db.writer();
    conn.execute(
        "DELETE FROM metadata_candidates WHERE id = ?1",
        params![candidate_id],
    )?;
    Ok(())
}

pub fn set_document_metadata_verified(
    db: &Database,
    document_id: i64,
    verified: bool,
) -> Result<DocumentMetadataView> {
    {
        let conn = db.writer();
        let tx = conn.unchecked_transaction()?;
        let now = chrono::Utc::now().timestamp();
        let changed = tx.execute(
            "UPDATE documents SET metadata_verified = ?2, updated_at = ?3 WHERE id = ?1",
            params![document_id, verified as i64, now],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!("document {document_id}")));
        }
        if verified {
            clear_metadata_jobs_for_doc(&tx, document_id)?;
        }

        let hashes: Vec<String> = {
            let mut st =
                tx.prepare("SELECT DISTINCT blake3_hash FROM files WHERE document_id = ?1")?;
            let iter = st.query_map(params![document_id], |r| r.get(0))?;
            iter.filter_map(std::result::Result::ok).collect()
        };
        for h in hashes {
            if !archive_if_manual(&tx, document_id, &h)? {
                tx.execute(
                    "DELETE FROM manual_metadata_archive WHERE blake3_hash = ?1",
                    params![h],
                )?;
            }
        }
        tx.commit()?;
    }
    get_document_metadata(db, document_id)
}

pub fn get_document_metadata(db: &Database, document_id: i64) -> Result<DocumentMetadataView> {
    let conn = db.reader();
    let row = conn.query_row(
        "SELECT work_type, title, journal_abbrev, journal_full, booktitle, series, volume,
                issue_number, year, pages, edition, publisher, address, isbn, issn, doi,
                arxiv_id, arxiv_version, mr_number, mr_number_display, mr_reviewer, zbl_number,
                url, msc_primary, msc_primary_status, abstract_text, raw_mrlookup_bibtex,
                metadata_source, metadata_license_note, manual_locked_fields, exclude_from_cloud_llm,
                metadata_verified
         FROM documents WHERE id = ?1",
        params![document_id],
        |r| {
            let locked_raw: String = r.get(29)?;
            let locked: Vec<String> = serde_json::from_str(&locked_raw).unwrap_or_default();
            Ok(DocumentMetadataView {
                document_id,
                work_type: r.get(0)?,
                title: r.get(1)?,
                authors: Vec::new(),
                journal_abbrev: r.get(2)?,
                journal_full: r.get(3)?,
                booktitle: r.get(4)?,
                series: r.get(5)?,
                volume: r.get(6)?,
                issue_number: r.get(7)?,
                year: r.get(8)?,
                pages: r.get(9)?,
                edition: r.get(10)?,
                publisher: r.get(11)?,
                address: r.get(12)?,
                isbn: r.get(13)?,
                issn: r.get(14)?,
                doi: r.get(15)?,
                arxiv_id: r.get(16)?,
                arxiv_version: r.get(17)?,
                mr_number: r.get(18)?,
                mr_number_display: r.get(19)?,
                mr_reviewer: r.get(20)?,
                zbl_number: r.get(21)?,
                url: r.get(22)?,
                msc_primary: r.get(23)?,
                msc_primary_status: r.get(24)?,
                msc_secondary: Vec::new(),
                abstract_text: r.get(25)?,
                raw_mrlookup_bibtex: r.get(26)?,
                metadata_source: r.get(27)?,
                metadata_license_note: r.get(28)?,
                manual_locked_fields: locked,
                exclude_from_cloud_llm: r.get::<_, i64>(30)? != 0,
                metadata_verified: r.get::<_, i64>(31)? != 0,
                tags: Vec::new(),
                pending_candidates: Vec::new(),
            })
        },
    ).optional()?
    .ok_or_else(|| CoreError::NotFound(format!("document {document_id}")))?;

    let mut doc = row;

    {
        let mut st = conn.prepare(
            "SELECT family_name, given_name, role FROM document_authors
             WHERE document_id = ?1 ORDER BY role ASC, position ASC",
        )?;
        doc.authors = st
            .query_map(params![document_id], |r| {
                Ok(BibliographicAuthor {
                    family_name: r.get(0)?,
                    given_name: r.get(1)?,
                    role: r.get(2)?,
                })
            })?
            .filter_map(std::result::Result::ok)
            .collect();
    }

    {
        let mut st = conn.prepare(
            "SELECT msc_code FROM document_msc_secondary WHERE document_id = ?1 ORDER BY msc_code ASC",
        )?;
        doc.msc_secondary = st
            .query_map(params![document_id], |r| r.get(0))?
            .filter_map(std::result::Result::ok)
            .collect();
    }

    {
        let mut st = conn.prepare(
            "SELECT t.name FROM tags t
             JOIN document_tags dt ON dt.tag_id = t.id
             WHERE dt.document_id = ?1 ORDER BY t.name ASC",
        )?;
        doc.tags = st
            .query_map(params![document_id], |r| r.get(0))?
            .filter_map(std::result::Result::ok)
            .collect();
    }

    {
        let mut st = conn.prepare(
            "SELECT id, source, match_score, candidate_json FROM metadata_candidates
             WHERE document_id = ?1 ORDER BY match_score DESC",
        )?;
        doc.pending_candidates = st
            .query_map(params![document_id], |r| {
                let json: String = r.get(3)?;
                let record: BibliographicRecord = serde_json::from_str(&json).unwrap_or_default();
                Ok(CandidateRow {
                    id: r.get(0)?,
                    document_id,
                    source: r.get(1)?,
                    match_score: r.get::<_, f64>(2)? as f32,
                    record,
                })
            })?
            .filter_map(std::result::Result::ok)
            .collect();
    }

    Ok(doc)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetadataSearchFilter {
    pub text: Option<String>,
    pub author: Option<String>,
    pub title: Option<String>,
    pub journal: Option<String>,
    pub year_from: Option<i64>,
    pub year_to: Option<i64>,
    pub msc_prefix: Option<String>,
    pub tag: Option<String>,
    #[serde(default)]
    pub cluster_id: Option<i64>,
    pub root_id: Option<String>,
    pub format: Option<String>,
    #[serde(default)]
    pub work_category: Option<String>, // "all" | "paper" | "book"
    #[serde(default)]
    pub verified_filter: Option<String>, // "all" | "verified" | "unverified"
    pub sort_by: Option<String>, // "updated" | "title" | "year" | "author"
    #[serde(default)]
    pub sort_dir: Option<String>, // "asc" | "desc"
}

/// Fast accent/transliteration-tolerant metadata search targeting `<= 100 ms` on 5,000 docs.
pub fn search_documents_metadata(
    conn: &Connection,
    filter: &MetadataSearchFilter,
) -> Result<Vec<crate::db::repos::docs::DocumentListItem>> {
    use crate::db::repos::docs::DocumentListItem;

    let sort_key = filter
        .sort_by
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "updated".to_string());
    let default_asc = matches!(sort_key.as_str(), "title" | "author");
    let asc = match filter
        .sort_dir
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("asc") => true,
        Some("desc") => false,
        _ => default_asc,
    };
    let dir_sql = if asc { "ASC" } else { "DESC" };

    let order_clause = match sort_key.as_str() {
        "title" => format!(
            "CASE WHEN TRIM(d.title_norm) = '' THEN 1 ELSE 0 END ASC, d.title_norm {dir_sql}, d.id DESC"
        ),
        "year" => format!(
            "CASE WHEN d.year IS NULL THEN 1 ELSE 0 END ASC, d.year {dir_sql}, d.title_norm ASC"
        ),
        "author" => format!(
            "CASE WHEN TRIM(d.authors_norm_summary) = '' THEN 1 ELSE 0 END ASC, d.authors_norm_summary {dir_sql}, d.title_norm ASC"
        ),
        _ => format!("d.updated_at {dir_sql}, d.id {dir_sql}"),
    };

    let cat_clause = match filter
        .work_category
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("book") => "AND d.work_type = 'book'",
        Some("paper") => "AND d.work_type != 'book'",
        _ => "",
    };

    let verified_clause = match filter
        .verified_filter
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("verified") => "AND d.metadata_verified != 0",
        Some("unverified") => "AND d.metadata_verified = 0",
        _ => "",
    };

    let sql = format!(
        "SELECT d.id, f.id, f.root_id, sr.label, d.work_type, d.title,
                COALESCE(
                    (SELECT GROUP_CONCAT(
                        CASE WHEN TRIM(given_name) = '' THEN family_name
                             WHEN TRIM(family_name) = '' THEN given_name
                             ELSE family_name || ', ' || given_name END,
                        '; '
                     )
                     FROM (
                        SELECT family_name, given_name
                        FROM document_authors
                        WHERE document_id = d.id
                        ORDER BY CASE WHEN role = 'author' THEN 0 ELSE 1 END, position ASC
                     )),
                    NULLIF(d.authors_norm_summary, ''),
                    ''
                ) AS authors_display,
                d.authors_norm_summary,
                d.year, f.format, f.rel_path, f.page_count, f.is_available,
                f.index_state, f.error_message,
                COALESCE(d.journal_full, '') || ' ' || COALESCE(d.journal_abbrev, '') || ' ' || COALESCE(d.booktitle, ''),
                COALESCE(d.msc_primary, ''),
                d.msc_primary_status,
                f.has_text_layer,
                f.ocr_status,
                d.metadata_source,
                d.metadata_verified
         FROM files f
         JOIN documents d ON d.id = f.document_id
         JOIN source_roots sr ON sr.id = f.root_id
         WHERE (?1 IS NULL OR f.root_id = ?1)
           AND (?2 IS NULL OR f.format = ?2)
           AND (?3 IS NULL OR d.year >= ?3)
           AND (?4 IS NULL OR d.year <= ?4)
           AND (?5 IS NULL OR (
                UPPER(COALESCE(d.msc_primary, '')) LIKE UPPER(?5) || '%'
                OR EXISTS (
                    SELECT 1 FROM document_msc_secondary dms
                    WHERE dms.document_id = d.id AND UPPER(dms.msc_code) LIKE UPPER(?5) || '%'
                )
           ))
           AND (?6 IS NULL OR EXISTS (
                SELECT 1 FROM document_tags dt
                JOIN tags t ON t.id = dt.tag_id
                WHERE dt.document_id = d.id AND LOWER(t.name) = LOWER(?6)
           ))
           AND (?7 IS NULL OR EXISTS (
                SELECT 1 FROM document_clusters dc
                WHERE dc.document_id = d.id AND dc.cluster_id = ?7
           ))
           {cat_clause}
           {verified_clause}
         GROUP BY d.id
         ORDER BY {order_clause}"
    );

    let text_q = norm_opt(&filter.text);
    let author_q = norm_opt(&filter.author);
    let title_q = norm_opt(&filter.title);
    let journal_q = norm_opt(&filter.journal);

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            clean_opt(&filter.root_id),
            clean_opt(&filter.format),
            filter.year_from,
            filter.year_to,
            clean_opt(&filter.msc_prefix),
            clean_opt(&filter.tag),
            filter.cluster_id,
        ],
        |r| {
            Ok((
                DocumentListItem {
                    document_id: r.get(0)?,
                    file_id: r.get(1)?,
                    root_id: r.get(2)?,
                    root_label: r.get(3)?,
                    work_type: r.get(4)?,
                    title: r.get(5)?,
                    authors_summary: r.get(6)?,
                    year: r.get(8)?,
                    format: r.get(9)?,
                    rel_path: r.get(10)?,
                    page_count: r.get::<_, Option<i64>>(11)?.map(|v| v as u32),
                    is_available: r.get::<_, i64>(12)? != 0,
                    index_state: r.get(13)?,
                    error_message: r.get(14)?,
                    msc_primary: {
                        let raw: String = r.get(16)?;
                        if raw.trim().is_empty() { None } else { Some(raw) }
                    },
                    msc_primary_status: r.get(17)?,
                    has_text_layer: r.get::<_, i64>(18)? != 0,
                    ocr_status: r.get(19)?,
                    metadata_source: r.get(20)?,
                    metadata_verified: r.get::<_, i64>(21)? != 0,
                },
                r.get::<_, String>(7)?,
                r.get::<_, String>(15)?,
                r.get::<_, String>(16)?,
            ))
        },
    )?;

    let mut out = Vec::new();
    for r in rows {
        let (item, raw_authors_norm, journal_raw, msc_raw) = r?;
        let t_norm = normalize_math_text(&item.title);
        let a_disp_norm = normalize_math_text(&item.authors_summary);
        let j_norm = normalize_math_text(&journal_raw);
        let m_norm = msc_raw.to_ascii_lowercase();

        if let Some(ref q) = title_q {
            if !t_norm.contains(q) {
                continue;
            }
        }
        if let Some(ref q) = author_q {
            if !a_disp_norm.contains(q) && !raw_authors_norm.contains(q) {
                continue;
            }
        }
        if let Some(ref q) = journal_q {
            if !j_norm.contains(q) {
                continue;
            }
        }
        if let Some(ref q) = text_q {
            let yr_str = item.year.map(|y| y.to_string()).unwrap_or_default();
            if !t_norm.contains(q)
                && !a_disp_norm.contains(q)
                && !raw_authors_norm.contains(q)
                && !j_norm.contains(q)
                && !m_norm.contains(q)
                && !yr_str.contains(q)
            {
                continue;
            }
        }
        out.push(item);
    }

    // Accent/diacritic-normalized alphabetical sort by first author's surname + given name,
    // keeping documents without authors at the bottom regardless of asc/desc direction.
    if sort_key == "author" {
        let mut keyed: Vec<(String, String, DocumentListItem)> = out
            .into_iter()
            .map(|it| {
                let ak = normalize_math_text(&it.authors_summary);
                let tk = normalize_math_text(&it.title);
                (ak, tk, it)
            })
            .collect();
        keyed.sort_by(|a, b| match (a.0.is_empty(), b.0.is_empty()) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => {
                let cmp = if asc {
                    a.0.cmp(&b.0)
                } else {
                    b.0.cmp(&a.0)
                };
                cmp.then_with(|| a.1.cmp(&b.1))
            }
        });
        out = keyed.into_iter().map(|(_, _, it)| it).collect();
    }

    Ok(out)
}

fn norm_opt(opt: &Option<String>) -> Option<String> {
    opt.as_deref()
        .map(normalize_math_text)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn clean_opt(opt: &Option<String>) -> Option<String> {
    opt.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn split_heuristic_author(raw: &str) -> BibliographicAuthor {
    let trimmed = raw.trim();
    if let Some((fam, giv)) = trimmed.split_once(',') {
        BibliographicAuthor {
            family_name: fam.trim().to_string(),
            given_name: giv.trim().to_string(),
            role: "author".to_string(),
        }
    } else {
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 2 {
            BibliographicAuthor {
                family_name: parts.last().unwrap_or(&"").to_string(),
                given_name: parts[..parts.len() - 1].join(" "),
                role: "author".to_string(),
            }
        } else {
            BibliographicAuthor {
                family_name: trimmed.to_string(),
                given_name: String::new(),
                role: "author".to_string(),
            }
        }
    }
}

pub fn list_all_document_ids(conn: &Connection) -> Result<Vec<i64>> {
    let mut st = conn.prepare("SELECT id FROM documents ORDER BY updated_at DESC, id DESC")?;
    let iter = st.query_map([], |row| row.get::<_, i64>(0))?;
    Ok(iter.filter_map(std::result::Result::ok).collect())
}

fn find_matching_document_for_record(conn: &Connection, rec: &BibliographicRecord) -> Option<i64> {
    if let Some(doi) = rec.doi.as_deref().filter(|s| !s.is_empty()) {
        if let Ok(Some(id)) = conn
            .query_row(
                "SELECT id FROM documents WHERE lower(doi) = lower(?1) LIMIT 1",
                params![doi],
                |r| r.get(0),
            )
            .optional()
        {
            return Some(id);
        }
    }
    if let Some(arx) = rec.arxiv_id.as_deref().filter(|s| !s.is_empty()) {
        if let Ok(Some(id)) = conn
            .query_row(
                "SELECT id FROM documents WHERE arxiv_id = ?1 LIMIT 1",
                params![arx],
                |r| r.get(0),
            )
            .optional()
        {
            return Some(id);
        }
    }
    if let Some(mr) = rec.mr_number.as_deref().filter(|s| !s.is_empty()) {
        if let Ok(Some(id)) = conn
            .query_row(
                "SELECT id FROM documents WHERE mr_number = ?1 LIMIT 1",
                params![mr],
                |r| r.get(0),
            )
            .optional()
        {
            return Some(id);
        }
    }
    if let Some(isbn) = rec.isbn.as_deref().filter(|s| !s.is_empty()) {
        if let Ok(Some(id)) = conn
            .query_row(
                "SELECT id FROM documents WHERE isbn = ?1 LIMIT 1",
                params![isbn],
                |r| r.get(0),
            )
            .optional()
        {
            return Some(id);
        }
    }
    let title_norm = normalize_math_text(&rec.title);
    if !title_norm.is_empty() {
        if let Ok(Some(id)) = conn
            .query_row(
                "SELECT id FROM documents WHERE title_norm = ?1 LIMIT 1",
                params![title_norm],
                |r| r.get(0),
            )
            .optional()
        {
            return Some(id);
        }
    }
    None
}

pub fn import_metadata_records_into_library(
    db: &Database,
    records: &[BibliographicRecord],
    target_document_id: Option<i64>,
) -> Result<usize> {
    let conn = db.writer();
    let mut matched = 0usize;
    for rec in records {
        let doc_id = if records.len() == 1 {
            target_document_id.or_else(|| find_matching_document_for_record(&conn, rec))
        } else {
            find_matching_document_for_record(&conn, rec)
        };
        if let Some(id) = doc_id {
            apply_record_respecting_locks(&conn, id, rec, false)?;
            matched += 1;
        }
    }
    Ok(matched)
}

pub fn clear_metadata_jobs_for_doc(conn: &Connection, document_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM index_jobs WHERE stage = 'metadata' AND file_id IN (SELECT id FROM files WHERE document_id = ?1)",
        params![document_id],
    )?;
    Ok(())
}

pub fn list_documents_for_bulk_metadata(
    conn: &Connection,
    only_unresolved: bool,
) -> Result<Vec<(i64, String)>> {
    let sql = if only_unresolved {
        "SELECT d.id, d.title
         FROM documents d
         WHERE d.metadata_verified = 0
           AND COALESCE(d.metadata_source, 'heuristic') = 'heuristic'
         ORDER BY d.id ASC"
    } else {
        "SELECT d.id, d.title
         FROM documents d
         WHERE d.metadata_verified = 0
         ORDER BY d.id ASC"
    };

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub async fn run_pending_metadata_jobs(
    db: &Database,
    clients: &WaterfallClients,
    config: &AppConfig,
    limit: usize,
) -> Result<usize> {
    if !config.network_lookups_enabled {
        return Ok(0);
    }
    // Discard any queued background metadata jobs for already-confirmed documents
    {
        let w = db.writer();
        let _ = w.execute(
            "DELETE FROM index_jobs
             WHERE stage = 'metadata' AND status = 'queued'
               AND file_id IN (
                   SELECT f.id FROM files f
                   JOIN documents d ON d.id = f.document_id
                   WHERE COALESCE(d.metadata_verified, 0) != 0
               )",
            [],
        );
    }
    let pending: Vec<(i64, i64)> = {
        let r = db.reader();
        let mut st = r.prepare(
            "SELECT j.id, f.document_id FROM index_jobs j
             JOIN files f ON f.id = j.file_id
             JOIN documents d ON d.id = f.document_id
             WHERE j.stage = 'metadata' AND j.status = 'queued'
               AND COALESCE(d.metadata_verified, 0) = 0
             ORDER BY j.priority ASC, j.id ASC LIMIT ?1",
        )?;
        let iter = st.query_map(params![limit as i64], |row| Ok((row.get(0)?, row.get(1)?)))?;
        iter.filter_map(std::result::Result::ok).collect()
    };

    let mut processed = 0usize;
    for (job_id, doc_id) in pending {
        let outcome = resolve_document_metadata(
            db,
            clients,
            config,
            doc_id,
            MetadataLookupScope::BulkBackgroundScan,
        )
        .await;
        let w = db.writer();
        match outcome {
            Ok(_) => {
                let _ = w.execute("DELETE FROM index_jobs WHERE id = ?1", params![job_id]);
                processed += 1;
            }
            Err(e) => {
                let _ = w.execute(
                    "UPDATE index_jobs SET status = 'failed', attempts = attempts + 1, last_error = ?1 WHERE id = ?2",
                    params![e.to_string(), job_id],
                );
            }
        }
    }
    Ok(processed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repos::docs::{create_heuristic_document, insert_file_record, NewFileRecord};
    use crate::db::repos::roots::{insert_source_root, NewSourceRoot};
    use tempfile::tempdir;

    #[test]
    fn search_documents_metadata_filters_work_category_and_sorts_alphabetically() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("meta_filter.db"), 2).unwrap();
        let root_path = dir.path().join("docs");
        std::fs::create_dir_all(&root_path).unwrap();

        {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Library".into(),
                    root_path: root_path.to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();

            let add_doc = |title: &str,
                               work_type: &str,
                               family: Option<&str>,
                               given: &str,
                               year: Option<i64>,
                               rel: &str| {
                let doc_id = create_heuristic_document(&w, title).unwrap();
                w.execute(
                    "UPDATE documents SET work_type = ?1, year = ?2 WHERE id = ?3",
                    params![work_type, year, doc_id],
                )
                .unwrap();
                if let Some(fam) = family {
                    replace_authors_in_tx(
                        &w,
                        doc_id,
                        &[BibliographicAuthor {
                            family_name: fam.to_string(),
                            given_name: given.to_string(),
                            role: "author".to_string(),
                        }],
                    )
                    .unwrap();
                }
                insert_file_record(
                    &w,
                    &NewFileRecord {
                        document_id: doc_id,
                        root_id: "r1",
                        rel_path: rel,
                        format: "pdf",
                        size_bytes: 100,
                        mtime_ns: 0,
                        blake3_hash: rel,
                        index_state: "bm25_done",
                    },
                )
                .unwrap();
            };

            add_doc(
                "Zeta Functions on Varieties",
                "article",
                Some("Grothendieck"),
                "Alexander",
                Some(1964),
                "groth.pdf",
            );
            add_doc(
                "Algebraic Geometry",
                "book",
                Some("Hartshorne"),
                "Robin",
                Some(1977),
                "hartshorne.pdf",
            );
            add_doc(
                "Mixed Finite Elements for Saddle Point Problems",
                "article",
                Some("Brezzi"),
                "Franco",
                Some(1974),
                "brezzi.pdf",
            );
            add_doc(
                "Approximation by Polynomials",
                "article",
                Some("Čebyšev"),
                "Pafnuty",
                Some(1854),
                "cebysev.pdf",
            );
            add_doc(
                "Notes Without Author",
                "book",
                None,
                "",
                None,
                "anon_book.pdf",
            );
        }

        let r = db.reader();

        // 1. Filter only papers vs only books
        let papers = search_documents_metadata(
            &r,
            &MetadataSearchFilter {
                work_category: Some("paper".into()),
                sort_by: Some("title".into()),
                sort_dir: Some("asc".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(papers.len(), 3);
        assert!(papers.iter().all(|d| d.work_type != "book"));
        assert_eq!(papers[0].title, "Approximation by Polynomials");
        assert_eq!(papers[1].title, "Mixed Finite Elements for Saddle Point Problems");
        assert_eq!(papers[2].title, "Zeta Functions on Varieties");

        let books = search_documents_metadata(
            &r,
            &MetadataSearchFilter {
                work_category: Some("book".into()),
                sort_by: Some("title".into()),
                sort_dir: Some("desc".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(books.len(), 2);
        assert!(books.iter().all(|d| d.work_type == "book"));
        assert_eq!(books[0].title, "Notes Without Author");
        assert_eq!(books[1].title, "Algebraic Geometry");

        // 2. Sort by author ascending and descending (empty authors stay last)
        let by_author_asc = search_documents_metadata(
            &r,
            &MetadataSearchFilter {
                sort_by: Some("author".into()),
                sort_dir: Some("asc".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let asc_authors: Vec<&str> = by_author_asc.iter().map(|d| d.authors_summary.as_str()).collect();
        assert_eq!(
            asc_authors,
            vec![
                "Brezzi, Franco",
                "Čebyšev, Pafnuty",
                "Grothendieck, Alexander",
                "Hartshorne, Robin",
                "",
            ]
        );

        let by_author_desc = search_documents_metadata(
            &r,
            &MetadataSearchFilter {
                sort_by: Some("author".into()),
                sort_dir: Some("desc".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let desc_authors: Vec<&str> = by_author_desc.iter().map(|d| d.authors_summary.as_str()).collect();
        assert_eq!(
            desc_authors,
            vec![
                "Hartshorne, Robin",
                "Grothendieck, Alexander",
                "Čebyšev, Pafnuty",
                "Brezzi, Franco",
                "",
            ]
        );
    }

    #[test]
    fn guided_lookup_uses_only_locked_fields_and_replaces_wrong_unlocked_metadata() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("guided.db"), 2).unwrap();
        let root_path = dir.path().join("docs");
        std::fs::create_dir_all(&root_path).unwrap();
        let cfg = AppConfig::default();

        let doc_id = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Library".into(),
                    root_path: root_path.to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();

            let doc_id = create_heuristic_document(&w, "Wrong Conference Paper").unwrap();
            w.execute(
                "UPDATE documents SET
                    work_type = 'inproceedings',
                    journal_full = 'Wrong Journal',
                    booktitle = 'Proceedings of Wrong Conf',
                    year = 2019,
                    doi = '10.9999/wrong.2019',
                    raw_mrlookup_bibtex = '@inproceedings{wrong2019, title={Wrong}}',
                    metadata_source = 'crossref'
                 WHERE id = ?1",
                params![doc_id],
            )
            .unwrap();
            replace_authors_in_tx(
                &w,
                doc_id,
                &[BibliographicAuthor {
                    family_name: "Smith".into(),
                    given_name: "John".into(),
                    role: "author".into(),
                }],
            )
            .unwrap();
            let file_id = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: doc_id,
                    root_id: "r1",
                    rel_path: "brezzi1974.pdf",
                    format: "pdf",
                    size_bytes: 100,
                    mtime_ns: 0,
                    blake3_hash: "hash_brezzi",
                    index_state: "bm25_done",
                },
            )
            .unwrap();
            // Noisy passage containing a cited paper's DOI on page 1
            w.execute(
                "INSERT INTO passages (document_id, file_id, page_ord, chunk_index, token_count, text_content, bboxes_blob)
                 VALUES (?1, ?2, 1, 0, 8, 'See also doi:10.1234/cited.noisy.paper for background.', X'')",
                params![doc_id, file_id],
            )
            .unwrap();
            doc_id
        };

        // Case 1: User enters & locks only the Title, resetting wrong unlocked fields
        let edit_title = ManualMetadataEdit {
            document_id: doc_id,
            work_type: "inproceedings".into(),
            title: "On the existence, uniqueness and approximation of saddle-point problems".into(),
            authors: vec![BibliographicAuthor {
                family_name: "Smith".into(),
                given_name: "John".into(),
                role: "author".into(),
            }],
            journal_abbrev: None,
            journal_full: Some("Wrong Journal".into()),
            booktitle: Some("Proceedings of Wrong Conf".into()),
            series: None,
            volume: None,
            issue_number: None,
            year: Some(2019),
            pages: None,
            edition: None,
            publisher: None,
            address: None,
            isbn: None,
            issn: None,
            doi: Some("10.9999/wrong.2019".into()),
            arxiv_id: None,
            mr_number: None,
            mr_reviewer: None,
            zbl_number: None,
            url: None,
            msc_primary: None,
            msc_secondary: vec![],
            abstract_text: None,
            exclude_from_cloud_llm: false,
            tags: vec!["fem".into()],
            edited_fields: vec!["title".into()],
        };

        let after_reset = apply_manual_metadata_edit_with_locks(
            &db,
            &edit_title,
            Some(&["title".to_string()]),
            true,
        )
        .unwrap();

        assert_eq!(after_reset.manual_locked_fields, vec!["title".to_string()]);
        assert!(after_reset.authors.is_empty());
        assert_eq!(after_reset.journal_full, None);
        assert_eq!(after_reset.booktitle, None);
        assert_eq!(after_reset.doi, None);
        assert_eq!(after_reset.year, None);
        assert_eq!(after_reset.raw_mrlookup_bibtex, None);
        assert_eq!(after_reset.tags, vec!["fem".to_string()]);

        let ids = gather_document_identifiers(&db, &cfg, doc_id).unwrap();
        assert_eq!(
            ids.heuristic_title.as_deref(),
            Some("On the existence, uniqueness and approximation of saddle-point problems")
        );
        // Must ignore cited DOI in PDF text and cleared wrong metadata
        assert_eq!(ids.doi, None);
        assert_eq!(ids.heuristic_first_author, None);
        assert_eq!(ids.heuristic_year, None);

        // Apply resolved online record -> fills unlocked fields while keeping locked user title
        let resolved_rec = BibliographicRecord {
            work_type: WorkType::Article,
            title: "On the existence, uniqueness and approximation of saddle-point problems arising from Lagrangian multipliers".into(),
            authors: vec![BibliographicAuthor {
                family_name: "Brezzi".into(),
                given_name: "Franco".into(),
                role: "author".into(),
            }],
            journal_abbrev: Some("RAIRO Anal. Numér.".into()),
            journal_full: Some("Revue Française d'Automatique, Informatique, Recherche Opérationnelle. Analyse Numérique".into()),
            volume: Some("8".into()),
            issue_number: Some("R-2".into()),
            year: Some(1974),
            pages: Some("129--151".into()),
            mr_number: Some("0365287".into()),
            zbl_number: Some("0302.65067".into()),
            msc_primary: Some("65N30".into()),
            source: "zbmath".into(),
            ..Default::default()
        };
        {
            let w = db.writer();
            apply_record_respecting_locks(&w, doc_id, &resolved_rec, false).unwrap();
        }
        let final_meta = get_document_metadata(&db, doc_id).unwrap();
        // Locked title kept verbatim
        assert_eq!(
            final_meta.title,
            "On the existence, uniqueness and approximation of saddle-point problems"
        );
        // Remaining unlocked fields populated cleanly
        assert_eq!(final_meta.authors.len(), 1);
        assert_eq!(final_meta.authors[0].family_name, "Brezzi");
        assert_eq!(final_meta.journal_abbrev.as_deref(), Some("RAIRO Anal. Numér."));
        assert_eq!(final_meta.booktitle, None);
        assert_eq!(final_meta.year, Some(1974));
        assert_eq!(final_meta.pages.as_deref(), Some("129--151"));
        assert_eq!(final_meta.mr_number.as_deref(), Some("0365287"));
        assert_eq!(final_meta.metadata_source.as_deref(), Some("zbmath"));

        // Case 2: User unlocks Title and locks ONLY Authors ("De Giorgi, Ennio")
        let mut edit_author_only = edit_title.clone();
        edit_author_only.authors = vec![BibliographicAuthor {
            family_name: "De Giorgi".into(),
            given_name: "Ennio".into(),
            role: "author".into(),
        }];
        edit_author_only.edited_fields = vec!["authors".into()];
        let after_author_lock = apply_manual_metadata_edit_with_locks(
            &db,
            &edit_author_only,
            Some(&["authors".to_string()]),
            true,
        )
        .unwrap();
        assert_eq!(after_author_lock.manual_locked_fields, vec!["authors".to_string()]);
        assert_eq!(after_author_lock.title, "brezzi1974");
        let ids_author = gather_document_identifiers(&db, &cfg, doc_id).unwrap();
        assert_eq!(ids_author.heuristic_title, None);
        assert_eq!(ids_author.heuristic_first_author.as_deref(), Some("De Giorgi, Ennio"));
    }

    #[tokio::test]
    async fn resolve_document_metadata_respects_custom_source_priority_and_fallback() {
        use crate::metadata::limiter::store_http_cache;

        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("prio.db"), 2).unwrap();
        let mut cfg = AppConfig {
            crossref_enabled: false,
            openalex_enabled: false,
            datacite_enabled: false,
            inspirehep_enabled: false,
            arxiv_enabled: false,
            semanticscholar_enabled: false,
            europepmc_enabled: false,
            openlibrary_enabled: false,
            ..Default::default()
        };
        let clients = WaterfallClients::from_config(&cfg).unwrap();

        let title = "On the existence uniqueness and approximation of saddle point problems";
        let mrlookup_cache_key = format!("q:ti={}:au=brezzi:yr=0", normalize_math_text(title));
        let zbmath_cache_key = format!("q:{}", normalize_math_text(&format!("{title} Brezzi")));

        let mr_html = r#"<html><body><pre>
@article {MR0365287,
    AUTHOR = {Brezzi, Franco},
     TITLE = {On the existence, uniqueness and approximation of saddle-point problems arising from {L}agrangian multipliers},
   JOURNAL = {Rev. Fran\c{c}aise Automat. Informat. Recherche Op\'{e}rationnelle S\'{e}r. Rouge},
    VOLUME = {8},
      YEAR = {1974},
     PAGES = {129--151},
  MRCLASS = {65N30},
  MRNUMBER = {0365287},
}
</pre></body></html>"#;

        let zb_json = r#"{
            "result": [{
                "id": 30265067,
                "identifier": "0302.65067",
                "title": {"title": "On the existence, uniqueness and approximation of saddle-point problems arising from Lagrangian multipliers"},
                "contributors": {"authors": [{"name": "Brezzi, Franco"}]},
                "year": "1974",
                "msc": [{"code": "65N30"}],
                "source": {"series": [{"short_title": "RAIRO Anal. Numer.", "title": "RAIRO Analyse Numerique", "volume": "8", "pages": "129-151"}]}
            }]
        }"#;

        store_http_cache(&db, "mrlookup", &mrlookup_cache_key, 200, mr_html).unwrap();
        store_http_cache(&db, "zbmath", &zbmath_cache_key, 200, zb_json).unwrap();

        let make_locked_doc = || {
            let doc_id = {
                let w = db.writer();
                create_heuristic_document(&w, title).unwrap()
            };
            apply_manual_metadata_edit_with_locks(
                &db,
                &ManualMetadataEdit {
                    document_id: doc_id,
                    work_type: "article".into(),
                    title: title.into(),
                    authors: vec![BibliographicAuthor {
                        family_name: "Brezzi".into(),
                        given_name: "Franco".into(),
                        role: "author".into(),
                    }],
                    journal_abbrev: None,
                    journal_full: None,
                    booktitle: None,
                    series: None,
                    volume: None,
                    issue_number: None,
                    year: None,
                    pages: None,
                    edition: None,
                    publisher: None,
                    address: None,
                    isbn: None,
                    issn: None,
                    doi: None,
                    arxiv_id: None,
                    mr_number: None,
                    mr_reviewer: None,
                    zbl_number: None,
                    url: None,
                    msc_primary: None,
                    msc_secondary: vec![],
                    abstract_text: None,
                    exclude_from_cloud_llm: false,
                    tags: vec![],
                    edited_fields: vec!["title".into(), "authors".into()],
                },
                Some(&["title".into(), "authors".into()]),
                true,
            )
            .unwrap();
            doc_id
        };

        // 1. MRLookup prioritized ahead of zbMATH -> MRLookup wins
        cfg.metadata_source_priority = vec!["mrlookup".into(), "zbmath".into()];
        let doc1 = make_locked_doc();
        let out1 = resolve_document_metadata(
            &db,
            &clients,
            &cfg,
            doc1,
            MetadataLookupScope::SingleRecordUserAction,
        )
        .await
        .unwrap();
        assert_eq!(out1.status, "updated");
        assert_eq!(out1.source.as_deref(), Some("mrlookup"));
        let meta1 = get_document_metadata(&db, doc1).unwrap();
        assert_eq!(meta1.mr_number.as_deref(), Some("0365287"));

        // 2. zbMATH prioritized ahead of MRLookup -> zbMATH wins
        cfg.metadata_source_priority = vec!["zbmath".into(), "mrlookup".into()];
        let doc2 = make_locked_doc();
        let out2 = resolve_document_metadata(
            &db,
            &clients,
            &cfg,
            doc2,
            MetadataLookupScope::SingleRecordUserAction,
        )
        .await
        .unwrap();
        assert_eq!(out2.status, "updated");
        assert_eq!(out2.source.as_deref(), Some("zbmath"));
        let meta2 = get_document_metadata(&db, doc2).unwrap();
        assert_eq!(meta2.zbl_number.as_deref(), Some("0302.65067"));

        // 3. MRLookup #1 returns empty -> automatically falls back to #2 zbMATH
        store_http_cache(&db, "mrlookup", &mrlookup_cache_key, 200, "<html><body>No matches</body></html>").unwrap();
        cfg.metadata_source_priority = vec!["mrlookup".into(), "zbmath".into()];
        let doc3 = make_locked_doc();
        let out3 = resolve_document_metadata(
            &db,
            &clients,
            &cfg,
            doc3,
            MetadataLookupScope::SingleRecordUserAction,
        )
        .await
        .unwrap();
        assert_eq!(out3.status, "updated");
        assert_eq!(out3.source.as_deref(), Some("zbmath"));
    }

    #[test]
    fn set_document_metadata_verified_persists_filters_and_skips_bulk_scan() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("verified.db"), 2).unwrap();
        let root_path = dir.path().join("docs");
        std::fs::create_dir_all(&root_path).unwrap();

        let (doc_a, doc_b) = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Lib".into(),
                    root_path: root_path.to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();
            let a = create_heuristic_document(&w, "Paper Alpha").unwrap();
            let b = create_heuristic_document(&w, "Paper Beta").unwrap();
            for (doc_id, rel, hash) in [(a, "alpha.pdf", "hash_ver_0"), (b, "beta.pdf", "hash_ver_1")] {
                insert_file_record(
                    &w,
                    &NewFileRecord {
                        document_id: doc_id,
                        root_id: "r1",
                        rel_path: rel,
                        format: "pdf",
                        size_bytes: 100,
                        mtime_ns: 1,
                        blake3_hash: hash,
                        index_state: "bm25_done",
                    },
                )
                .unwrap();
            }
            (a, b)
        };

        let meta_a = set_document_metadata_verified(&db, doc_a, true).unwrap();
        assert!(meta_a.metadata_verified);
        assert!(!get_document_metadata(&db, doc_b).unwrap().metadata_verified);

        let r = db.reader();
        let verified_only = search_documents_metadata(
            &r,
            &MetadataSearchFilter {
                verified_filter: Some("verified".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(verified_only.len(), 1);
        assert_eq!(verified_only[0].document_id, doc_a);
        assert!(verified_only[0].metadata_verified);

        let unverified_only = search_documents_metadata(
            &r,
            &MetadataSearchFilter {
                verified_filter: Some("unverified".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(unverified_only.len(), 1);
        assert_eq!(unverified_only[0].document_id, doc_b);
        assert!(!unverified_only[0].metadata_verified);

        // Verified document is skipped by bulk metadata scans even if heuristic or force-all
        let bulk_unres = list_documents_for_bulk_metadata(&r, true).unwrap();
        assert_eq!(bulk_unres.len(), 1);
        assert_eq!(bulk_unres[0].0, doc_b);
        let bulk_all = list_documents_for_bulk_metadata(&r, false).unwrap();
        assert_eq!(bulk_all.len(), 1);
        assert_eq!(bulk_all[0].0, doc_b);
        drop(r);

        // Unchecking restores normal state and cleans empty archive
        let meta_a2 = set_document_metadata_verified(&db, doc_a, false).unwrap();
        assert!(!meta_a2.metadata_verified);
        let r2 = db.reader();
        assert_eq!(list_documents_for_bulk_metadata(&r2, true).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn verified_metadata_cannot_be_modified_by_search_or_extraction() {
        use crate::db::repos::docs::enqueue_index_job;

        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("verified_protect.db"), 2).unwrap();
        let root_path = dir.path().join("docs");
        std::fs::create_dir_all(&root_path).unwrap();
        let pdf_file = root_path.join("confirmed.pdf");
        std::fs::write(&pdf_file, b"%PDF-1.4\n%mock\n").unwrap();

        let doc_id = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Docs".into(),
                    root_path: root_path.to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();
            let d = create_heuristic_document(&w, "Confirmed Custom Title").unwrap();
            let f_id = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: d,
                    root_id: "r1",
                    rel_path: "confirmed.pdf",
                    format: "pdf",
                    size_bytes: 15,
                    mtime_ns: 1,
                    blake3_hash: "hash-confirmed",
                    index_state: "bm25_done",
                },
            )
            .unwrap();
            enqueue_index_job(&w, f_id, "metadata", 10).unwrap();
            d
        };

        set_document_metadata_verified(&db, doc_id, true).unwrap();

        // Queued metadata job must be cleared on verify and skipped by background runner
        let remaining_jobs: i64 = db
            .reader()
            .query_row(
                "SELECT COUNT(*) FROM index_jobs WHERE stage = 'metadata'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining_jobs, 0);

        // Local extraction must NOT overwrite verified title/DOI/ISBN even on heuristic source
        {
            let w = db.writer();
            let ids = ExtractedIdentifiers {
                heuristic_title: Some("Extracted Different Title".into()),
                doi: Some("10.1000/overwrite-attempt".into()),
                isbn: Some("9780387902449".into()),
                ..Default::default()
            };
            apply_extracted_identifiers_to_doc(&w, doc_id, &ids, None).unwrap();

            let external_rec = BibliographicRecord {
                source: "crossref".into(),
                title: "Remote Overwrite Title".into(),
                doi: Some("10.1000/remote".into()),
                year: Some(2005),
                ..Default::default()
            };
            apply_record_respecting_locks(&w, doc_id, &external_rec, false).unwrap();
        }

        let cfg = AppConfig::default();
        let clients = WaterfallClients::from_config(&cfg).unwrap();
        let outcome = resolve_document_metadata(
            &db,
            &clients,
            &cfg,
            doc_id,
            MetadataLookupScope::SingleRecordUserAction,
        )
        .await
        .unwrap();
        assert_eq!(outcome.status, "verified_locked");

        let processed = run_pending_metadata_jobs(&db, &clients, &cfg, 10).await.unwrap();
        assert_eq!(processed, 0);

        let after = get_document_metadata(&db, doc_id).unwrap();
        assert_eq!(after.title, "Confirmed Custom Title");
        assert!(after.doi.is_none());
        assert!(after.year.is_none());
        assert!(after.metadata_verified);
    }

    #[tokio::test]
    async fn resolve_document_metadata_bridges_page1_doi_into_mrlookup_and_heals_stale_doi() {
        use crate::metadata::limiter::store_http_cache;

        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("doi_bridge.db"), 2).unwrap();
        let root_path = dir.path().join("docs");
        std::fs::create_dir_all(&root_path).unwrap();

        let cfg = AppConfig {
            metadata_source_priority: vec!["mrlookup".into(), "crossref".into()],
            zbmath_enabled: false,
            dblp_enabled: false,
            openalex_enabled: false,
            datacite_enabled: false,
            inspirehep_enabled: false,
            arxiv_enabled: false,
            semanticscholar_enabled: false,
            europepmc_enabled: false,
            openlibrary_enabled: false,
            ..Default::default()
        };
        let clients = WaterfallClients::from_config(&cfg).unwrap();

        let doc_id = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Lib".into(),
                    root_path: root_path.to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();
            let doc_id = create_heuristic_document(&w, "scan_001").unwrap();
            // Simulate a previously truncated Elsevier SICI DOI stuck in documents.doi
            w.execute(
                "UPDATE documents SET doi = '10.1016/s0022-1236(02' WHERE id = ?1",
                params![doc_id],
            )
            .unwrap();
            let file_id = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: doc_id,
                    root_id: "r1",
                    rel_path: "scan_001.pdf",
                    format: "pdf",
                    size_bytes: 100,
                    mtime_ns: 1,
                    blake3_hash: "hash_els_doi",
                    index_state: "bm25_done",
                },
            )
            .unwrap();
            // Page 1 has blatant Elsevier DOI with parentheses, even though header layout is messy
            let p1 = "JOURNAL OF FUNCTIONAL ANALYSIS 190 (2002) 1-42\n\
                      AVAILABLE ONLINE AT HTTP://WWW.IDEALIBRARY.COM\n\
                      doi:10.1016/S0022-1236(02)00045-2\n\
                      Sobolev Spaces on Metric Measure Spaces — Piotr Hajlasz\n\
                      Abstract. We establish sharp Sobolev embeddings on general metric measure spaces.";
            w.execute(
                "INSERT INTO passages (document_id, file_id, page_ord, chunk_index, token_count, text_content, bboxes_blob)
                 VALUES (?1, ?2, 1, 0, 30, ?3, X'')",
                params![doc_id, file_id, p1],
            )
            .unwrap();
            doc_id
        };

        let full_doi = "10.1016/s0022-1236(02)00045-2";
        let cr_doi_json = r#"{
            "status": "ok",
            "message": {
                "DOI": "10.1016/s0022-1236(02)00045-2",
                "type": "journal-article",
                "title": ["Sobolev spaces on metric-measure spaces"],
                "author": [{"family": "Hajlasz", "given": "Piotr"}],
                "container-title": ["Journal of Functional Analysis"],
                "volume": "190",
                "page": "1-42",
                "published-print": {"date-parts": [[2002]]}
            }
        }"#;
        store_http_cache(&db, "crossref", &format!("doi:{full_doi}"), 200, cr_doi_json).unwrap();

        // MRLookup receives the clean bridged title + author from Crossref and wins at #1!
        let mr_ck = format!(
            "q:ti={}:au=hajlasz:yr=0",
            normalize_math_text("Sobolev spaces on metric-measure spaces")
        );
        let mr_html = r#"<html><body><pre>
@article {MR1906745,
    AUTHOR = {Haj\l asz, Piotr},
     TITLE = {Sobolev spaces on metric-measure spaces},
   JOURNAL = {J. Funct. Anal.},
    VOLUME = {190},
      YEAR = {2002},
     PAGES = {1--42},
   MRCLASS = {46E35},
  MRNUMBER = {1906745},
}
</pre></body></html>"#;
        store_http_cache(&db, "mrlookup", &mr_ck, 200, mr_html).unwrap();

        let out = resolve_document_metadata(
            &db,
            &clients,
            &cfg,
            doc_id,
            MetadataLookupScope::SingleRecordUserAction,
        )
        .await
        .unwrap();
        assert_eq!(out.status, "updated");
        assert_eq!(out.source.as_deref(), Some("mrlookup"));

        let meta = get_document_metadata(&db, doc_id).unwrap();
        assert_eq!(meta.mr_number.as_deref(), Some("1906745"));
        assert_eq!(meta.msc_primary.as_deref(), Some("46E35"));
        // DOI healed and merged from Crossref bridge even though MRLookup BibTeX lacked DOI!
        assert_eq!(meta.doi.as_deref(), Some(full_doi));
    }

    #[tokio::test]
    async fn resolve_document_metadata_title_only_succeeds_with_preprint_and_published_duplicates() {
        use crate::metadata::limiter::store_http_cache;

        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("title_only.db"), 2).unwrap();
        let cfg = AppConfig {
            mrlookup_enabled_single: false,
            zbmath_enabled: false,
            crossref_enabled: true,
            dblp_enabled: false,
            openalex_enabled: false,
            datacite_enabled: false,
            inspirehep_enabled: false,
            arxiv_enabled: false,
            semanticscholar_enabled: false,
            europepmc_enabled: false,
            openlibrary_enabled: false,
            ..Default::default()
        };
        let clients = WaterfallClients::from_config(&cfg).unwrap();

        let title = "Discontinuous Galerkin Methods on Polytopic Meshes";
        let doc_id = {
            let w = db.writer();
            create_heuristic_document(&w, title).unwrap()
        };
        // User locks ONLY the title (zero authors)
        apply_manual_metadata_edit_with_locks(
            &db,
            &ManualMetadataEdit {
                document_id: doc_id,
                work_type: "article".into(),
                title: title.into(),
                authors: vec![],
                journal_abbrev: None,
                journal_full: None,
                booktitle: None,
                series: None,
                volume: None,
                issue_number: None,
                year: None,
                pages: None,
                edition: None,
                publisher: None,
                address: None,
                isbn: None,
                issn: None,
                doi: None,
                arxiv_id: None,
                mr_number: None,
                mr_reviewer: None,
                zbl_number: None,
                url: None,
                msc_primary: None,
                msc_secondary: vec![],
                abstract_text: None,
                exclude_from_cloud_llm: false,
                tags: vec![],
                edited_fields: vec!["title".into()],
            },
            Some(&["title".into()]),
            true,
        )
        .unwrap();

        // Crossref returns BOTH a preprint (listed first!) AND the published journal version with identical title
        let cr_list_json = r#"{
            "status": "ok",
            "message": {
                "items": [
                    {
                        "DOI": "10.2139/ssrn.999999",
                        "type": "posted-content",
                        "title": ["Discontinuous Galerkin Methods on Polytopic Meshes"],
                        "author": [{"family": "Antonietti", "given": "Paola F."}],
                        "published-online": {"date-parts": [[2020]]}
                    },
                    {
                        "DOI": "10.1007/s00211-021-01234-5",
                        "type": "journal-article",
                        "title": ["Discontinuous Galerkin Methods on Polytopic Meshes"],
                        "author": [
                            {"family": "Antonietti", "given": "Paola F."},
                            {"family": "Cangiani", "given": "Andrea"}
                        ],
                        "container-title": ["Numerische Mathematik"],
                        "volume": "149",
                        "page": "345-382",
                        "published-print": {"date-parts": [[2021]]}
                    },
                    {
                        "DOI": "10.1007/unrelated",
                        "type": "journal-article",
                        "title": ["Finite Differences on Structured Grids"],
                        "author": [{"family": "Other", "given": "A."}],
                        "published-print": {"date-parts": [[2018]]}
                    }
                ]
            }
        }"#;
        let ck = format!("q:{}", normalize_math_text(title));
        store_http_cache(&db, "crossref", &ck, 200, cr_list_json).unwrap();

        let out = resolve_document_metadata(
            &db,
            &clients,
            &cfg,
            doc_id,
            MetadataLookupScope::SingleRecordUserAction,
        )
        .await
        .unwrap();
        assert_eq!(out.status, "updated");
        assert_eq!(out.source.as_deref(), Some("crossref"));

        let meta = get_document_metadata(&db, doc_id).unwrap();
        // Published Numerische Mathematik version wins over the preprint!
        assert_eq!(meta.journal_full.as_deref(), Some("Numerische Mathematik"));
        assert_eq!(meta.volume.as_deref(), Some("149"));
        assert_eq!(meta.pages.as_deref(), Some("345--382"));
        assert_eq!(meta.doi.as_deref(), Some("10.1007/s00211-021-01234-5"));
        assert_eq!(meta.authors.len(), 2);
    }

    #[tokio::test]
    async fn resolve_document_metadata_blocks_bibliography_doi_and_false_positive_candidates() {
        use crate::metadata::limiter::store_http_cache;

        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("anti_fp.db"), 2).unwrap();
        let root_path = dir.path().join("docs");
        std::fs::create_dir_all(&root_path).unwrap();

        let cfg = AppConfig {
            mrlookup_enabled_single: false,
            zbmath_enabled: false,
            crossref_enabled: true,
            dblp_enabled: false,
            openalex_enabled: false,
            datacite_enabled: false,
            inspirehep_enabled: false,
            arxiv_enabled: false,
            semanticscholar_enabled: false,
            europepmc_enabled: false,
            openlibrary_enabled: false,
            ..Default::default()
        };
        let clients = WaterfallClients::from_config(&cfg).unwrap();

        let doc_id = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Lib".into(),
                    root_path: root_path.to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();
            let doc_id = create_heuristic_document(&w, "preprint").unwrap();
            let file_id = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: doc_id,
                    root_id: "r1",
                    rel_path: "preprint.pdf",
                    format: "pdf",
                    size_bytes: 100,
                    mtime_ns: 1,
                    blake3_hash: "hash_anti_fp",
                    index_state: "bm25_done",
                },
            )
            .unwrap();
            let p1 = "Optimal Error Estimates for Mixed Finite Elements\n\
                      Franco Brezzi\n\
                      Abstract. We prove optimal convergence rates in L2 and H(div) norms for mixed elements.";
            let p18_bib = "References\n\
                           [12] X. TotallyWrong, Quantum Black Holes in AdS, doi:10.1007/cited-biblio-doi, 2020.";
            w.execute(
                "INSERT INTO passages (document_id, file_id, page_ord, chunk_index, token_count, text_content, bboxes_blob)
                 VALUES (?1, ?2, 1, 0, 25, ?3, X''), (?1, ?2, 18, 0, 15, ?4, X'')",
                params![doc_id, file_id, p1, p18_bib],
            )
            .unwrap();
            doc_id
        };

        // Even if Crossref had the cited bibliography DOI cached AND returned a paper with similar title
        // by a completely different author ("TotallyWrong") not present anywhere on pages 1-2:
        let cited_json = r#"{
            "status": "ok",
            "message": {
                "DOI": "10.1007/cited-biblio-doi",
                "type": "journal-article",
                "title": ["Quantum Black Holes in AdS"],
                "author": [{"family": "TotallyWrong", "given": "X."}]
            }
        }"#;
        store_http_cache(&db, "crossref", "doi:10.1007/cited-biblio-doi", 200, cited_json).unwrap();

        let wrong_author_list = r#"{
            "status": "ok",
            "message": {
                "items": [{
                    "DOI": "10.1007/wrong-author",
                    "type": "journal-article",
                    "title": ["Optimal Error Estimates for Mixed Finite Elements"],
                    "author": [{"family": "NonExistentAuthor", "given": "Z."}],
                    "published-print": {"date-parts": [[1975]]}
                }]
            }
        }"#;
        let q_ck = format!(
            "q:{}",
            normalize_math_text("Optimal Error Estimates for Mixed Finite Elements Brezzi")
        );
        let q_no_au_ck = format!(
            "q:{}",
            normalize_math_text("Optimal Error Estimates for Mixed Finite Elements")
        );
        store_http_cache(&db, "crossref", &q_ck, 200, wrong_author_list).unwrap();
        store_http_cache(&db, "crossref", &q_no_au_ck, 200, wrong_author_list).unwrap();

        let out = resolve_document_metadata(
            &db,
            &clients,
            &cfg,
            doc_id,
            MetadataLookupScope::SingleRecordUserAction,
        )
        .await
        .unwrap();

        // Must NOT auto-update to either the bibliography paper or the wrong-author paper!
        assert_eq!(out.status, "unchanged");
        let meta = get_document_metadata(&db, doc_id).unwrap();
        assert_eq!(meta.doi, None);
        assert_eq!(meta.title, "Optimal Error Estimates for Mixed Finite Elements");
    }

    #[tokio::test]
    async fn resolve_document_metadata_bridges_elsevier_pii_and_doiorg_csl_and_fuses_fields() {
        use crate::metadata::limiter::store_http_cache;

        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("wave2_bridge.db"), 2).unwrap();
        let root_path = dir.path().join("docs");
        std::fs::create_dir_all(&root_path).unwrap();

        let cfg = AppConfig {
            metadata_source_priority: vec!["mrlookup".into(), "crossref".into()],
            mrlookup_enabled_single: true,
            zbmath_enabled: false,
            crossref_enabled: true,
            dblp_enabled: false,
            openalex_enabled: false,
            datacite_enabled: true,
            inspirehep_enabled: false,
            arxiv_enabled: false,
            semanticscholar_enabled: false,
            europepmc_enabled: false,
            openlibrary_enabled: false,
            ..Default::default()
        };
        let clients = WaterfallClients::from_config(&cfg).unwrap();

        // Subtest A: ScienceDirect filename `1-s2.0-S0021999118307125-main.pdf` with NO DOI on page 1
        // and OT1 ligatures ("Diﬀusion") -> Crossref `alternative-id` PII bridge resolves canonical title + DOI,
        // #1 MRLookup wins, and Cross-Source Field Fusion attaches DOI + abstract to MRLookup's record!
        let doc_pii = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Lib".into(),
                    root_path: root_path.to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();
            let doc_id = create_heuristic_document(&w, "1-s2.0-S0021999118307125-main").unwrap();
            let file_id = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: doc_id,
                    root_id: "r1",
                    rel_path: "1-s2.0-S0021999118307125-main.pdf",
                    format: "pdf",
                    size_bytes: 100,
                    mtime_ns: 1,
                    blake3_hash: "hash_pii_1",
                    index_state: "bm25_done",
                },
            )
            .unwrap();
            let p1 = "Discontinuous Galerkin Schemes for Diﬀusion Equations\n\
                      Franco Brezzi and L. Donatella Marini\n\
                      Abstract. We analyze discontinuous Galerkin discretizations for elliptic diffusion.";
            w.execute(
                "INSERT INTO passages (document_id, file_id, page_ord, chunk_index, token_count, text_content, bboxes_blob)
                 VALUES (?1, ?2, 1, 0, 22, ?3, X'')",
                params![doc_id, file_id, p1],
            )
            .unwrap();
            doc_id
        };

        let pii_crossref_json = r#"{
            "status": "ok",
            "message": {
                "items": [{
                    "DOI": "10.1016/j.jcp.2018.10.042",
                    "type": "journal-article",
                    "title": ["Discontinuous Galerkin Schemes for Diffusion Equations"],
                    "author": [
                        {"family": "Brezzi", "given": "Franco"},
                        {"family": "Marini", "given": "L. Donatella"}
                    ],
                    "container-title": ["Journal of Computational Physics"],
                    "volume": "378",
                    "page": "120-144",
                    "abstract": "<jats:p>Canonical abstract from Crossref.</jats:p>",
                    "published-print": {"date-parts": [[2019]]}
                }]
            }
        }"#;
        store_http_cache(&db, "crossref", "pii:S0021999118307125", 200, pii_crossref_json).unwrap();

        let mr_ck = format!(
            "q:ti={}:au=brezzi:yr=0",
            normalize_math_text("Discontinuous Galerkin Schemes for Diffusion Equations")
        );
        let mr_html = r#"<html><body><pre>
@article {MR3881234,
    AUTHOR = {Brezzi, Franco and Marini, L. Donatella},
     TITLE = {Discontinuous {G}alerkin schemes for diffusion equations},
   JOURNAL = {J. Comput. Phys.},
  FJOURNAL = {Journal of Computational Physics},
    VOLUME = {378},
      YEAR = {2019},
     PAGES = {120--144},
   MRCLASS = {65N30},
  MRNUMBER = {3881234},
}
</pre></body></html>"#;
        store_http_cache(&db, "mrlookup", &mr_ck, 200, mr_html).unwrap();

        let out_pii = resolve_document_metadata(
            &db,
            &clients,
            &cfg,
            doc_pii,
            MetadataLookupScope::SingleRecordUserAction,
        )
        .await
        .unwrap();
        assert_eq!(out_pii.status, "updated");
        assert_eq!(out_pii.source.as_deref(), Some("mrlookup"));
        let meta_pii = get_document_metadata(&db, doc_pii).unwrap();
        assert_eq!(meta_pii.mr_number.as_deref(), Some("3881234"));
        assert_eq!(meta_pii.doi.as_deref(), Some("10.1016/j.jcp.2018.10.042"));
        assert_eq!(
            meta_pii.abstract_text.as_deref(),
            Some("Canonical abstract from Crossref.")
        );

        // Subtest B: Non-Crossref registration agency DOI (e.g. ETH / DataCite / mEDRA) returning 404 on Crossref
        // resolves via International DOI Foundation `doi.org` CSL-JSON content negotiation!
        let doc_csl = {
            let w = db.writer();
            let doc_id = create_heuristic_document(&w, "eth_thesis").unwrap();
            let file_id = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: doc_id,
                    root_id: "r1",
                    rel_path: "eth_paper.pdf",
                    format: "pdf",
                    size_bytes: 100,
                    mtime_ns: 1,
                    blake3_hash: "hash_csl_1",
                    index_state: "bm25_done",
                },
            )
            .unwrap();
            let p1 = "Adaptive Wavelet Methods for Boundary Integral Equations\n\
                      Christoph Schwab\n\
                      DOI: 10.3929/ethz-b-000123456\n\
                      Abstract. We study wavelet compression of boundary integral operators.";
            w.execute(
                "INSERT INTO passages (document_id, file_id, page_ord, chunk_index, token_count, text_content, bboxes_blob)
                 VALUES (?1, ?2, 1, 0, 22, ?3, X'')",
                params![doc_id, file_id, p1],
            )
            .unwrap();
            doc_id
        };

        store_http_cache(&db, "crossref", "doi:10.3929/ethz-b-000123456", 404, "Not found").unwrap();
        let csl_json = r#"{
            "type": "report",
            "DOI": "10.3929/ethz-b-000123456",
            "title": "Adaptive Wavelet Methods for Boundary Integral Equations",
            "author": [{"family": "Schwab", "given": "Christoph"}],
            "publisher": "ETH Zurich",
            "issued": {"date-parts": [[2004]]}
        }"#;
        store_http_cache(&db, "doiorg", "doi:10.3929/ethz-b-000123456", 200, csl_json).unwrap();
        // MRLookup does not index this technical report -> bridge_hit promotes CSL-JSON record directly!
        let mr_empty_ck = format!(
            "q:ti={}:au=schwab:yr=0",
            normalize_math_text("Adaptive Wavelet Methods for Boundary Integral Equations")
        );
        store_http_cache(&db, "mrlookup", &mr_empty_ck, 200, "<html></html>").unwrap();

        let mut cfg_no_dc = cfg.clone();
        cfg_no_dc.datacite_enabled = false;
        let out_csl = resolve_document_metadata(
            &db,
            &clients,
            &cfg_no_dc,
            doc_csl,
            MetadataLookupScope::SingleRecordUserAction,
        )
        .await
        .unwrap();
        assert_eq!(out_csl.status, "updated");
        let meta_csl = get_document_metadata(&db, doc_csl).unwrap();
        assert_eq!(
            meta_csl.title,
            "Adaptive Wavelet Methods for Boundary Integral Equations"
        );
        assert_eq!(meta_csl.doi.as_deref(), Some("10.3929/ethz-b-000123456"));
        assert_eq!(meta_csl.year, Some(2004));
    }

    #[test]
    fn verify_candidate_handles_multibyte_utf8_word_endings_without_panic() {
        let rec = BibliographicRecord {
            title:
                "Moduloλ cohomologyℓ spaces² дифференциальные уравнения 代数几何学研究 varietà Poincaré"
                    .to_string(),
            authors: vec![BibliographicAuthor {
                family_name: "Колмогоров".to_string(),
                given_name: "А.".to_string(),
                role: "author".to_string(),
            }],
            ..Default::default()
        };
        let cand = CandidateMatch {
            source: "crossref".to_string(),
            score: 0.92,
            record: rec,
        };
        let ctx = GatheredDocContext {
            ids: ExtractedIdentifiers::default(),
            db_title: None,
            verify_text: Some(normalize_math_text(
                "unrelated mathematical paper on harmonic analysis and partial differential operators in bounded domains with smooth boundary",
            )),
            is_guided: false,
            locked_title: false,
            locked_authors: false,
            locked_year: false,
        };

        assert!(!verify_candidate_against_context(&cand, &ctx));
        assert_eq!(word_stem_without_last_char("moduloλ"), Some("modulo"));
        assert_eq!(word_stem_without_last_char("代数几何"), None);
    }

    #[tokio::test]
    async fn final_title_only_double_check_resolves_when_author_or_year_noise_broke_pass1_and_guards_collisions() {
        use crate::metadata::limiter::store_http_cache;

        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("title_double_check.db"), 2).unwrap();
        let root_path = dir.path().join("docs");
        std::fs::create_dir_all(&root_path).unwrap();

        let cfg = AppConfig {
            mrlookup_enabled_single: false,
            zbmath_enabled: false,
            crossref_enabled: false,
            dblp_enabled: true,
            openalex_enabled: false,
            datacite_enabled: false,
            inspirehep_enabled: false,
            arxiv_enabled: false,
            semanticscholar_enabled: false,
            europepmc_enabled: false,
            openlibrary_enabled: false,
            ..Default::default()
        };
        let clients = WaterfallClients::from_config(&cfg).unwrap();

        // Case 1: PDF front matter has the true title & author (`Saddle Point Problems`), yet page-1 layout
        // yields a noisy heuristic author (`NoisyUniversityHeader`) so Pass 1 DBLP `"{title} {surname}"` fails.
        // The final title-only double-check must retry DBLP with ONLY the title, confirm the exact title match
        // (and verify `Brezzi` against PDF front matter), and update the metadata automatically!
        let (doc_noisy_author, doc_scanned_collision, doc_partial_title) = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Lib".into(),
                    root_path: root_path.to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();

            // Doc 1: page 1 has noisy first author line, author Brezzi in body, exact title in DB & header.
            let d1 = create_heuristic_document(&w, "Preprint").unwrap();
            let f1 = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: d1,
                    root_id: "r1",
                    rel_path: "paper1.pdf",
                    format: "pdf",
                    size_bytes: 100,
                    mtime_ns: 1,
                    blake3_hash: "h1",
                    index_state: "bm25_done",
                },
            )
            .unwrap();
            let p1 = "On the Existence and Approximation of Saddle-Point Problems\n\
                      By Politecnico Milano\n\
                      Franco Brezzi\n\
                      Abstract. We establish inf-sup stability conditions for mixed finite elements.";
            w.execute(
                "INSERT INTO passages (document_id, file_id, page_ord, chunk_index, token_count, text_content, bboxes_blob)
                 VALUES (?1, ?2, 1, 0, 22, ?3, X'')",
                params![d1, f1, p1],
            )
            .unwrap();
            w.execute(
                "UPDATE documents SET title = ?1, title_norm = ?2 WHERE id = ?3",
                params![
                    "On the Existence and Approximation of Saddle-Point Problems",
                    normalize_math_text("On the Existence and Approximation of Saddle-Point Problems"),
                    d1
                ],
            )
            .unwrap();

            // Doc 2: scanned/textless doc where DB title collides across TWO distinct authors in DBLP -> must NOT auto-pick!
            let d2 = create_heuristic_document(&w, "On Harmonic Maps Between Riemannian Manifolds").unwrap();
            let f2 = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: d2,
                    root_id: "r1",
                    rel_path: "scan2.pdf",
                    format: "pdf",
                    size_bytes: 100,
                    mtime_ns: 2,
                    blake3_hash: "h2",
                    index_state: "bm25_done",
                },
            )
            .unwrap();
            w.execute(
                "INSERT INTO passages (document_id, file_id, page_ord, chunk_index, token_count, text_content, bboxes_blob)
                 VALUES (?1, ?2, 1, 0, 0, '', X'')",
                params![d2, f2],
            )
            .unwrap();

            // Doc 3: DB has Part I, DBLP returns only Part II -> NOT a total title match!
            let d3 = create_heuristic_document(&w, "Theory of Elliptic Operators Part I").unwrap();
            (d1, d2, d3)
        };

        // Pass 1 DBLP query with noisy extracted author returns empty hits, while title-only DBLP query succeeds!
        let noisy_ck = format!(
            "q:{}",
            normalize_math_text("On the Existence and Approximation of Saddle-Point Problems Politecnico")
        );
        let empty_dblp = r#"{"result":{"hits":{"hit":[]}}}"#;
        store_http_cache(&db, "dblp", &noisy_ck, 200, empty_dblp).unwrap();

        let clean_ck = format!(
            "q:{}",
            normalize_math_text("On the Existence and Approximation of Saddle-Point Problems")
        );
        let exact_dblp = r#"{
            "result": {
                "hits": {
                    "hit": [{
                        "info": {
                            "title": "On the Existence and Approximation of Saddle-Point Problems.",
                            "authors": {"author": [{"text": "Franco Brezzi"}]},
                            "venue": "RAIRO Anal. Numer.",
                            "year": "1974",
                            "doi": "10.1051/m2an/197408R201291"
                        }
                    }]
                }
            }
        }"#;
        store_http_cache(&db, "dblp", &clean_ck, 200, exact_dblp).unwrap();

        let out1 = resolve_document_metadata(
            &db,
            &clients,
            &cfg,
            doc_noisy_author,
            MetadataLookupScope::SingleRecordUserAction,
        )
        .await
        .unwrap();
        assert_eq!(out1.status, "updated");
        let meta1 = get_document_metadata(&db, doc_noisy_author).unwrap();
        assert_eq!(meta1.doi.as_deref(), Some("10.1051/m2an/197408r201291"));
        assert_eq!(meta1.year, Some(1974));
        assert_eq!(meta1.authors.len(), 1);
        assert_eq!(meta1.authors[0].family_name, "Brezzi");

        // Doc 2: exact title collision between two distinct authors on textless PDF -> must NOT auto-pick
        let coll_ck = format!(
            "q:{}",
            normalize_math_text("On Harmonic Maps Between Riemannian Manifolds")
        );
        let coll_dblp = r#"{
            "result": {
                "hits": {
                    "hit": [
                        {
                            "info": {
                                "title": "On Harmonic Maps Between Riemannian Manifolds",
                                "authors": {"author": [{"text": "James Eells"}]},
                                "year": "1964",
                                "doi": "10.2307/2373037"
                            }
                        },
                        {
                            "info": {
                                "title": "On Harmonic Maps Between Riemannian Manifolds",
                                "authors": {"author": [{"text": "Another Mathematician"}]},
                                "year": "1992",
                                "doi": "10.1007/other-harmonic"
                            }
                        }
                    ]
                }
            }
        }"#;
        store_http_cache(&db, "dblp", &coll_ck, 200, coll_dblp).unwrap();
        let out2 = resolve_document_metadata(
            &db,
            &clients,
            &cfg,
            doc_scanned_collision,
            MetadataLookupScope::SingleRecordUserAction,
        )
        .await
        .unwrap();
        assert_eq!(out2.status, "candidates_found");
        assert!(out2.candidates_count >= 2);
        let meta2 = get_document_metadata(&db, doc_scanned_collision).unwrap();
        assert_eq!(meta2.doi, None);

        // Doc 3: Part I vs Part II must be rejected by total-title verification
        let part_ck = format!(
            "q:{}",
            normalize_math_text("Theory of Elliptic Operators Part I")
        );
        let part_dblp = r#"{
            "result": {
                "hits": {
                    "hit": [{
                        "info": {
                            "title": "Theory of Elliptic Operators Part II",
                            "authors": {"author": [{"text": "H. Evans"}]},
                            "year": "1998",
                            "doi": "10.1007/wrong-part-ii"
                        }
                    }]
                }
            }
        }"#;
        store_http_cache(&db, "dblp", &part_ck, 200, part_dblp).unwrap();
        let out3 = resolve_document_metadata(
            &db,
            &clients,
            &cfg,
            doc_partial_title,
            MetadataLookupScope::SingleRecordUserAction,
        )
        .await
        .unwrap();
        assert_ne!(out3.status, "updated");
        let meta3 = get_document_metadata(&db, doc_partial_title).unwrap();
        assert_eq!(meta3.doi, None);

        // Doc 4: documents.title already has the exact title while page-1 text had a noisy header/garbled layout
        let doc_db_title = {
            let w = db.writer();
            let d4 = create_heuristic_document(
                &w,
                "Regularity of Weak Solutions to Parabolic Systems",
            )
            .unwrap();
            let f4 = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: d4,
                    root_id: "r1",
                    rel_path: "scan_noisy_p1.pdf",
                    format: "pdf",
                    size_bytes: 100,
                    mtime_ns: 4,
                    blake3_hash: "h4",
                    index_state: "bm25_done",
                },
            )
            .unwrap();
            let noisy_p1 = "PROCEEDINGS OF THE MATHEMATICAL SOCIETY VOL 12\n\
                            M. Giaquinta and E. Giusti\n\
                            Abstract. We study partial regularity of weak solutions of nonlinear parabolic systems.";
            w.execute(
                "INSERT INTO passages (document_id, file_id, page_ord, chunk_index, token_count, text_content, bboxes_blob)
                 VALUES (?1, ?2, 1, 0, 20, ?3, X'')",
                params![d4, f4, noisy_p1],
            )
            .unwrap();
            d4
        };
        let noisy_p1_ck = format!(
            "q:{}",
            normalize_math_text(
                "Abstract. We study partial regularity of weak solutions of nonlinear parabolic systems. Giaquinta"
            )
        );
        store_http_cache(&db, "dblp", &noisy_p1_ck, 200, empty_dblp).unwrap();
        let db_title_ck = format!(
            "q:{}",
            normalize_math_text("Regularity of Weak Solutions to Parabolic Systems")
        );
        let db_title_dblp = r#"{
            "result": {
                "hits": {
                    "hit": [{
                        "info": {
                            "title": "Regularity of Weak Solutions to Parabolic Systems",
                            "authors": {"author": [{"text": "Mariano Giaquinta"}, {"text": "Enrico Giusti"}]},
                            "year": "1983",
                            "doi": "10.1007/parabolic-regularity"
                        }
                    }]
                }
            }
        }"#;
        store_http_cache(&db, "dblp", &db_title_ck, 200, db_title_dblp).unwrap();
        let out4 = resolve_document_metadata(
            &db,
            &clients,
            &cfg,
            doc_db_title,
            MetadataLookupScope::SingleRecordUserAction,
        )
        .await
        .unwrap();
        assert_eq!(out4.status, "updated");
        let meta4 = get_document_metadata(&db, doc_db_title).unwrap();
        assert_eq!(meta4.doi.as_deref(), Some("10.1007/parabolic-regularity"));
        assert_eq!(meta4.year, Some(1983));
    }
}
