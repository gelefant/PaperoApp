use crate::error::Result;
use crate::text_norm::normalize_math_text;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, FAST, INDEXED, STORED,
    STRING,
};
use tantivy::tokenizer::{
    RemoveLongFilter, SimpleTokenizer, TextAnalyzer, Token, TokenFilter, TokenStream, Tokenizer,
};
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy, Term};

pub const MATH_ANALYZER_NAME: &str = "math_norm";

#[derive(Clone)]
struct MathNormalizeFilter;

impl TokenFilter for MathNormalizeFilter {
    type Tokenizer<T: Tokenizer> = MathNormalizeTokenizer<T>;

    fn transform<T: Tokenizer>(self, tokenizer: T) -> Self::Tokenizer<T> {
        MathNormalizeTokenizer { inner: tokenizer }
    }
}

#[derive(Clone)]
struct MathNormalizeTokenizer<T> {
    inner: T,
}

impl<T: Tokenizer> Tokenizer for MathNormalizeTokenizer<T> {
    type TokenStream<'a> = MathNormalizeStream<T::TokenStream<'a>>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        MathNormalizeStream {
            tail: self.inner.token_stream(text),
        }
    }
}

struct MathNormalizeStream<T> {
    tail: T,
}

impl<T: TokenStream> TokenStream for MathNormalizeStream<T> {
    fn advance(&mut self) -> bool {
        if !self.tail.advance() {
            return false;
        }
        // Zero-allocation in-place fast path for pure ASCII tokens without backticks.
        if self
            .tail
            .token()
            .text
            .bytes()
            .all(|b| b < 0x80 && b != b'`')
        {
            self.tail.token_mut().text.make_ascii_lowercase();
            return true;
        }
        let normalized = normalize_math_text(&self.tail.token().text);
        self.tail.token_mut().text = normalized;
        true
    }

    fn token(&self) -> &Token {
        self.tail.token()
    }

    fn token_mut(&mut self) -> &mut Token {
        self.tail.token_mut()
    }
}

#[derive(Debug, Clone)]
pub struct Bm25Passage {
    pub passage_id: u64,
    pub document_id: u64,
    pub file_id: u64,
    pub root_id: String,
    pub text_content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bm25Hit {
    pub passage_id: u64,
    pub document_id: u64,
    pub file_id: u64,
    pub score: f32,
}

pub struct Bm25Index {
    index: Index,
    reader: IndexReader,
    /// Lazily opened during write batches and dropped on `commit()` so Tantivy's ~30–50 MB
    /// writer arena and background merge threads consume 0 MB RAM at idle.
    writer: Mutex<Option<IndexWriter>>,
    f_passage_id: Field,
    f_document_id: Field,
    f_file_id: Field,
    f_root_id: Field,
    f_text_content: Field,
}

impl Bm25Index {
    pub fn open_or_create(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let (schema, f_passage_id, f_document_id, f_file_id, f_root_id, f_text_content) =
            build_schema();

        let index = if dir.join("meta.json").exists() {
            Index::open_in_dir(dir)?
        } else {
            Index::create_in_dir(dir, schema)?
        };
        Self::from_index(
            index,
            f_passage_id,
            f_document_id,
            f_file_id,
            f_root_id,
            f_text_content,
        )
    }

    pub fn in_ram() -> Result<Self> {
        let (schema, f_passage_id, f_document_id, f_file_id, f_root_id, f_text_content) =
            build_schema();
        let index = Index::create_in_ram(schema);
        Self::from_index(
            index,
            f_passage_id,
            f_document_id,
            f_file_id,
            f_root_id,
            f_text_content,
        )
    }

    fn from_index(
        index: Index,
        f_passage_id: Field,
        f_document_id: Field,
        f_file_id: Field,
        f_root_id: Field,
        f_text_content: Field,
    ) -> Result<Self> {
        let analyzer = TextAnalyzer::builder(SimpleTokenizer::default())
            .filter(RemoveLongFilter::limit(100))
            .filter(MathNormalizeFilter)
            .build();
        index.tokenizers().register(MATH_ANALYZER_NAME, analyzer);

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;

        Ok(Self {
            index,
            reader,
            writer: Mutex::new(None),
            f_passage_id,
            f_document_id,
            f_file_id,
            f_root_id,
            f_text_content,
        })
    }

    fn ensure_writer<'a>(
        &'a self,
        slot: &'a mut Option<IndexWriter>,
    ) -> Result<&'a mut IndexWriter> {
        if slot.is_none() {
            let w = self.index.writer_with_num_threads(2, 30_000_000)?;
            *slot = Some(w);
        }
        Ok(slot.as_mut().expect("writer initialized"))
    }

    pub fn add_passages(&self, passages: &[Bm25Passage]) -> Result<()> {
        if passages.is_empty() {
            return Ok(());
        }
        let mut guard = self.writer.lock().expect("bm25 writer mutex poisoned");
        let writer = self.ensure_writer(&mut guard)?;
        for p in passages {
            writer.delete_term(Term::from_field_u64(self.f_passage_id, p.passage_id));
            writer.add_document(doc!(
                self.f_passage_id => p.passage_id,
                self.f_document_id => p.document_id,
                self.f_file_id => p.file_id,
                self.f_root_id => p.root_id.as_str(),
                self.f_text_content => p.text_content.as_str(),
            ))?;
        }
        Ok(())
    }

    pub fn delete_passages(&self, passage_ids: &[u64]) -> Result<()> {
        if passage_ids.is_empty() {
            return Ok(());
        }
        let mut guard = self.writer.lock().expect("bm25 writer mutex poisoned");
        let writer = self.ensure_writer(&mut guard)?;
        for &pid in passage_ids {
            writer.delete_term(Term::from_field_u64(self.f_passage_id, pid));
        }
        Ok(())
    }

    pub fn delete_document(&self, document_id: u64) -> Result<()> {
        let mut guard = self.writer.lock().expect("bm25 writer mutex poisoned");
        let writer = self.ensure_writer(&mut guard)?;
        writer.delete_term(Term::from_field_u64(self.f_document_id, document_id));
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        let writer_opt = {
            let mut guard = self.writer.lock().expect("bm25 writer mutex poisoned");
            if let Some(ref mut w) = *guard {
                w.commit()?;
            }
            guard.take()
        };
        if let Some(w) = writer_opt {
            let _ = w.wait_merging_threads();
        }
        self.reader.reload()?;
        Ok(())
    }

    /// Commits any buffered changes and snapshots `src_dir` while holding the writer mutex
    /// so concurrent commits/merges cannot delete segment files mid-backup.
    pub fn commit_and_copy_dir(&self, src_dir: &Path, dst_dir: &Path) -> Result<()> {
        let mut guard = self.writer.lock().expect("bm25 writer mutex poisoned");
        if let Some(mut w) = guard.take() {
            w.commit()?;
            let _ = w.wait_merging_threads();
        }
        self.reader.reload()?;
        if src_dir.exists() {
            crate::backup::copy_dir_excluding_locks(src_dir, dst_dir)?;
        }
        Ok(())
    }

    pub fn search(
        &self,
        query_text: &str,
        root_ids: Option<&[String]>,
        limit: usize,
    ) -> Result<Vec<Bm25Hit>> {
        let trimmed = query_text.trim();
        if trimmed.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let mut parser = QueryParser::for_index(&self.index, vec![self.f_text_content]);
        parser.set_conjunction_by_default();
        let parsed = match parser.parse_query(trimmed) {
            Ok(q) => q,
            Err(_) => {
                // Fallback to lenient parse if user typed unbalanced quotes/operators
                let (lenient_q, _) = parser.parse_query_lenient(trimmed);
                lenient_q
            }
        };

        let final_query: Box<dyn Query> = if let Some(roots) = root_ids {
            if roots.is_empty() {
                return Ok(Vec::new());
            }
            let root_clauses: Vec<(Occur, Box<dyn Query>)> = roots
                .iter()
                .map(|r| {
                    let t = Term::from_field_text(self.f_root_id, r);
                    (
                        Occur::Should,
                        Box::new(TermQuery::new(t, IndexRecordOption::Basic)) as Box<dyn Query>,
                    )
                })
                .collect();
            let root_filter = BooleanQuery::new(root_clauses);
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, parsed),
                (Occur::Must, Box::new(root_filter)),
            ]))
        } else {
            parsed
        };

        let searcher = self.reader.searcher();
        let top_docs = searcher.search(&final_query, &TopDocs::with_limit(limit))?;

        let mut hits = Vec::with_capacity(top_docs.len());
        for (score, addr) in top_docs {
            // Read `passage_id` and `document_id` directly from mmap FAST columns when available
            // so searches avoid decompressing up to 300 LZ4 doc-store blocks!
            let seg = searcher.segment_reader(addr.segment_ord);
            let ff = seg.fast_fields();
            let fast_pid = ff
                .u64("passage_id")
                .ok()
                .and_then(|col| col.first(addr.doc_id));
            let fast_did = ff
                .u64("document_id")
                .ok()
                .and_then(|col| col.first(addr.doc_id));

            if let (Some(passage_id), Some(document_id)) = (fast_pid, fast_did) {
                let fast_fid = ff
                    .u64("file_id")
                    .ok()
                    .and_then(|col| col.first(addr.doc_id))
                    .unwrap_or(0);
                hits.push(Bm25Hit {
                    passage_id,
                    document_id,
                    file_id: fast_fid,
                    score,
                });
            } else {
                let retrieved: tantivy::TantivyDocument = searcher.doc(addr)?;
                let passage_id = retrieved
                    .get_first(self.f_passage_id)
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let document_id = retrieved
                    .get_first(self.f_document_id)
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let file_id = retrieved
                    .get_first(self.f_file_id)
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                hits.push(Bm25Hit {
                    passage_id,
                    document_id,
                    file_id,
                    score,
                });
            }
        }

        Ok(hits)
    }
}

fn build_schema() -> (Schema, Field, Field, Field, Field, Field) {
    let mut builder = Schema::builder();
    let f_passage_id = builder.add_u64_field("passage_id", INDEXED | STORED | FAST);
    let f_document_id = builder.add_u64_field("document_id", INDEXED | STORED | FAST);
    let f_file_id = builder.add_u64_field("file_id", INDEXED | STORED | FAST);
    let f_root_id = builder.add_text_field("root_id", STRING | STORED);

    let text_indexing = TextFieldIndexing::default()
        .set_tokenizer(MATH_ANALYZER_NAME)
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    // Do not duplicate full `text_content` inside Tantivy's doc-store (SQLite `passages` already stores it).
    let text_options = TextOptions::default().set_indexing_options(text_indexing);
    let f_text_content = builder.add_text_field("text_content", text_options);

    (
        builder.build(),
        f_passage_id,
        f_document_id,
        f_file_id,
        f_root_id,
        f_text_content,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_analyzer_matches_diacritics_and_transliterations() {
        let idx = Bm25Index::in_ram().unwrap();
        idx.add_passages(&[
            Bm25Passage {
                passage_id: 1,
                document_id: 10,
                file_id: 100,
                root_id: "root-a".into(),
                text_content: "A theorem of Paul Erdős and Eduard Čech on Łukasiewicz algebras.".into(),
            },
            Bm25Passage {
                passage_id: 2,
                document_id: 11,
                file_id: 101,
                root_id: "root-b".into(),
                text_content: "Spectral geometry via Poincaré and Hölder estimates.".into(),
            },
        ])
        .unwrap();
        idx.commit().unwrap();

        let hits = idx.search("Erdos Cech Lukasiewicz", None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].passage_id, 1);

        let hits2 = idx.search("Poincare Holder", None, 10).unwrap();
        assert_eq!(hits2.len(), 1);
        assert_eq!(hits2[0].passage_id, 2);

        // Root filtering
        let filtered = idx.search("Poincare", Some(&["root-a".into()]), 10).unwrap();
        assert!(filtered.is_empty());
    }
}
