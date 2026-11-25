use clap::{Parser, Subcommand};
use papero_core::config::{AppConfig, AppPaths};
use papero_core::db::repos::docs::{
    create_heuristic_document, insert_file_record, replace_file_pages_and_passages, FilePageInsert,
    NewFileRecord, PassageInsert,
};
use papero_core::db::repos::roots::{insert_source_root, NewSourceRoot};
use papero_core::db::Database;
use papero_core::extract::chunker::chunk_document;
use papero_core::extract::djvu::extract_djvu;
use papero_core::extract::pdf::extract_pdf;
use papero_core::fs::roots::{detect_doc_format, new_root_id};
use papero_core::fs::scanner::rescan_sources;
use papero_core::index::bm25::{Bm25Index, Bm25Passage};
use papero_core::index::hybrid::{
    hybrid_search, DocScoreAggregation, HybridSearchContext, HybridSearchOptions,
};
use papero_core::index::vector::VectorIndex;
use papero_core::ml::{
    current_process_rss_bytes, download_model_profile, profile_is_ready, resolved_onnxruntime_path,
    run_ml_worker_stdio, LazyModelSlot, ModelProfile, OnnxEmbedder, OnnxReranker,
};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "papero-eval", about = "Papero evaluation & benchmark CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Benchmarks initial scan and unchanged incremental rescan on N synthetic files.
    BenchRescan {
        #[arg(long, default_value_t = 5000)]
        count: usize,
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Runs extraction + chunking + BM25 indexing smoke check on a single PDF or DjVu file.
    SmokeExtract {
        file: PathBuf,
        #[arg(long)]
        pdfium: Option<PathBuf>,
    },
    /// Benchmarks 5,000-doc content search (with & without reranker) and idle RSS budget.
    BenchSearch {
        #[arg(long, default_value_t = 5000)]
        count: usize,
        #[arg(long, default_value = "light")]
        profile: String,
        #[arg(long)]
        models_dir: Option<PathBuf>,
        #[arg(long)]
        ort_lib: Option<PathBuf>,
        #[arg(long, default_value_t = 8)]
        queries: usize,
    },
    /// Evaluates Recall@5/10/20 and MRR across BM25-only, BM25+Vector, and Hybrid+Reranker.
    EvalHybrid {
        #[arg(long, default_value = "light")]
        profile: String,
        #[arg(long)]
        models_dir: Option<PathBuf>,
        #[arg(long)]
        ort_lib: Option<PathBuf>,
    },
    /// Downloads and verifies (SHA-256) Hugging Face ONNX models for a profile.
    DownloadModels {
        #[arg(long, default_value = "light")]
        profile: String,
        #[arg(long)]
        models_dir: Option<PathBuf>,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--ml-worker") {
        run_ml_worker_stdio()?;
        return Ok(());
    }
    let cli = Cli::parse();
    match cli.command {
        Commands::BenchRescan { count, dir } => bench_rescan(count, dir),
        Commands::SmokeExtract { file, pdfium } => smoke_extract(file, pdfium),
        Commands::BenchSearch {
            count,
            profile,
            models_dir,
            ort_lib,
            queries,
        } => bench_search(count, &profile, models_dir, ort_lib, queries),
        Commands::EvalHybrid {
            profile,
            models_dir,
            ort_lib,
        } => eval_hybrid(&profile, models_dir, ort_lib),
        Commands::DownloadModels {
            profile,
            models_dir,
        } => download_models(&profile, models_dir),
    }
}

fn download_models(
    profile_str: &str,
    models_dir_opt: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let profile = ModelProfile::from_str_lossy(profile_str);
    let paths = AppPaths::from_data_dir(AppPaths::default_app_dir());
    let models_dir = models_dir_opt.unwrap_or(paths.models_dir);
    println!("Downloading profile '{}' to {}...", profile.as_str(), models_dir.display());
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(download_model_profile(&models_dir, profile, |ev| {
        if ev.finished || ev.artifact_bytes_done == ev.artifact_bytes_total {
            println!(
                "  {} ({:.1}/{:.1} MB)",
                ev.artifact_name,
                ev.profile_bytes_done as f64 / (1024.0 * 1024.0),
                ev.profile_bytes_total as f64 / (1024.0 * 1024.0)
            );
        }
    }))?;
    println!("Verified SHA-256 for profile '{}'!", profile.as_str());
    Ok(())
}

fn bench_rescan(count: usize, custom_dir: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let base_dir = custom_dir.unwrap_or_else(|| tmp.path().to_path_buf());
    let lib_dir = base_dir.join("synthetic_library");
    let db_path = base_dir.join("papero.db");
    let tantivy_dir = base_dir.join("tantivy");
    fs::create_dir_all(&lib_dir)?;

    println!("Generating {count} synthetic PDF/DjVu files in {}...", lib_dir.display());
    let gen_start = Instant::now();
    for subdir_idx in 0..50 {
        fs::create_dir_all(lib_dir.join(format!("shelf_{subdir_idx:02}")))?;
    }
    for i in 0..count {
        let shelf = i % 50;
        let ext = if i % 5 == 0 { "djvu" } else { "pdf" };
        let path = lib_dir.join(format!("shelf_{shelf:02}/doc_{i:05}.{ext}"));
        let payload = format!("%PDF-1.4 synthetic doc {i} unique content\n");
        fs::write(path, payload)?;
    }
    println!("Generated {count} files in {:.3} s", gen_start.elapsed().as_secs_f64());

    let db = Database::open(&db_path, 4)?;
    let bm25 = Bm25Index::open_or_create(&tantivy_dir)?;
    let root_id = new_root_id();
    {
        let w = db.writer();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: root_id,
                label: "Synthetic".into(),
                root_path: lib_dir.to_string_lossy().into_owned(),
                is_network_or_cloud: false,
            },
        )?;
    }

    let t_init = Instant::now();
    let s1 = rescan_sources(&db, Some(&bm25), None, None)?;
    let init_s = t_init.elapsed().as_secs_f64();
    println!(
        "Initial scan: new={}, elapsed={:.3} s ({:.0} files/s)",
        s1.cnt_new,
        init_s,
        (count as f64) / init_s
    );

    let t_rescan = Instant::now();
    let s2 = rescan_sources(&db, Some(&bm25), None, None)?;
    let rescan_s = t_rescan.elapsed().as_secs_f64();
    println!(
        "Unchanged rescan ({count} files): unchanged={}, elapsed={:.4} s (target <= 10.0 s -> {})",
        s2.cnt_unchanged,
        rescan_s,
        if rescan_s <= 10.0 { "PASS" } else { "FAIL" }
    );

    Ok(())
}

fn smoke_extract(file: PathBuf, pdfium: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let fmt = detect_doc_format(&file).ok_or("unsupported extension (expected .pdf or .djvu/.djv)")?;
    let cfg = AppConfig::default();
    let t0 = Instant::now();
    let extracted = match fmt {
        "pdf" => extract_pdf(&file, pdfium.as_deref())?,
        "djvu" => extract_djvu(&file, cfg.djvused_bin.as_deref())?,
        _ => unreachable!(),
    };
    let chunks = chunk_document(&extracted, cfg.chunk_target_tokens, cfg.chunk_overlap_tokens);
    println!(
        "Extracted {} ({fmt}): pages={}, has_text_layer={}, outline_items={}, chunks={}, elapsed={:.3} s",
        file.display(),
        extracted.page_count,
        extracted.has_text_layer,
        extracted.outline.len(),
        chunks.len(),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

struct EvalCorpusItem {
    title: &'static str,
    text: &'static str,
    query: &'static str,
}

fn math_eval_corpus() -> Vec<EvalCorpusItem> {
    vec![
        EvalCorpusItem {
            title: "Banach Fixed-Point Principle in Complete Metric Spaces",
            text: "Let (X, d) be a complete metric space and T: X -> X a Lipschitz contraction with constant q < 1. Iterating x_{n+1} = T(x_n) yields a Cauchy sequence converging to the unique solution x* = T(x*), giving existence and uniqueness for integral equations.",
            query: "unique solution by iterating a contraction on a complete metric space",
        },
        EvalCorpusItem {
            title: "Hahn-Banach Theorem and Separating Hyperplanes",
            text: "Any sublinear bounded functional defined on a linear subspace of a real normed vector space can be extended to the whole Banach space without increasing its operator norm. Geometrically this separates disjoint closed convex sets.",
            query: "extending bounded linear functionals on normed spaces without increasing norm",
        },
        EvalCorpusItem {
            title: "Spectral Theorem for Compact Self-Adjoint Operators",
            text: "A compact Hermitian operator acting on a separable Hilbert space admits an orthonormal basis of eigenvectors whose real eigenvalues accumulate only at zero.",
            query: "orthonormal eigenbasis of a compact self-adjoint operator on Hilbert space",
        },
        EvalCorpusItem {
            title: "Euler Product Representation of the Riemann Zeta Function",
            text: "For Re(s) > 1, the Dirichlet series sum_{n>=1} n^{-s} equals the convergent product over all rational primes prod_p (1 - p^{-s})^{-1}. As s -> 1+, the harmonic pole implies the infinitude of prime numbers.",
            query: "product over prime numbers equal to Dirichlet series sum n^-s",
        },
        EvalCorpusItem {
            title: "Gauss-Bonnet Theorem on Compact Oriented Surfaces",
            text: "Integrating the Gaussian curvature K over a compact oriented Riemannian 2-manifold without boundary yields 2 pi times the Euler characteristic chi(M), linking local differential geometry to global topology.",
            query: "integral of Gaussian curvature equals 2 pi times Euler characteristic",
        },
        EvalCorpusItem {
            title: "Stokes Theorem on Differential Forms and Manifolds",
            text: "If omega is a smooth compactly supported (n-1)-form on an oriented n-manifold M with boundary dM, then int_M d(omega) = int_{dM} omega, unifying Green, Gauss divergence, and classical Kelvin-Stokes formulas.",
            query: "integral of exterior derivative of a form equals integral over boundary",
        },
        EvalCorpusItem {
            title: "Brouwer Fixed Point and Sperner Lemma",
            text: "Every continuous self-map of the closed n-dimensional Euclidean unit disk possesses at least one fixed point. The combinatorial proof derives from Sperner triangulation coloring and homology of spheres.",
            query: "continuous function from closed unit ball to itself leaves a point invariant",
        },
        EvalCorpusItem {
            title: "Prime Number Theorem via Tauberian Contour Integration",
            text: "The counting function pi(x) of primes up to x is asymptotic to x / ln x. Non-vanishing of zeta(1 + it) on the abscissa Re(s)=1 drives Newman's contour proof.",
            query: "asymptotic density x over log x of primes using non-vanishing on Re(s)=1",
        },
        EvalCorpusItem {
            title: "Galois Correspondence and Solvability by Radicals",
            text: "Intermediate subfields of a finite normal separable extension K/F correspond reversing inclusion to subgroups of Aut(K/F). A polynomial over F is solvable by radicals precisely when its Galois group is solvable.",
            query: "subfields correspond to subgroups and radical roots iff solvable group",
        },
        EvalCorpusItem {
            title: "Central Limit Theorem via Characteristic Functions",
            text: "Normalized partial sums (S_n - n mu)/(sigma sqrt(n)) of i.i.d. random variables with finite second moment converge in distribution to a standard Gaussian N(0,1), established by Taylor expanding phi(t/sqrt(n)) via Levy continuity.",
            query: "sum of independent random variables converges in distribution to normal Gaussian law",
        },
        EvalCorpusItem {
            title: "Sobolev Embedding and Rellich-Kondrachov Compactness",
            text: "On a bounded Lipschitz domain Omega in R^n, functions in W^{1,p}(Omega) belong continuously to L^{p*} and embed compactly into L^q for subcritical exponents q < np/(n-p), crucial for elliptic variational PDE.",
            query: "compact embedding of Sobolev space W^{1,p} into subcritical L^q on bounded domain",
        },
        EvalCorpusItem {
            title: "Cauchy Integral Formula for Holomorphic Functions",
            text: "If f is complex differentiable inside and on a positively oriented simple closed rectifiable contour gamma, then f(z_0) = (1/(2 pi i)) oint_gamma f(z)/(z - z_0) dz, implying infinite differentiability and analyticity.",
            query: "value of analytic complex function inside contour given by boundary integral",
        },
        EvalCorpusItem {
            title: "Martingale Convergence Theorem and Doob Upcrossing",
            text: "Every L^1-bounded submartingale converges almost surely as discrete time n -> infinity to an integrable limiting random variable, proved by bounding expected upcrossings over rational intervals [a,b].",
            query: "almost sure convergence of bounded submartingale proved with upcrossing inequality",
        },
        EvalCorpusItem {
            title: "Classification of Finitely Generated Abelian Groups",
            text: "Every finitely generated module over a principal ideal domain decomposes uniquely up to isomorphism into a free part Z^r plus cyclic torsion summands Z/d_i Z with d_1 | d_2 | ... | d_k via Smith Normal Form.",
            query: "invariant factors and Smith normal form decomposition of torsion plus free abelian group",
        },
        EvalCorpusItem {
            title: "Lax-Milgram Lemma for Weak Elliptic PDE Solutions",
            text: "Given a continuous coercive bilinear form a(u,v) on a real Hilbert space V and continuous linear functional F, there exists a unique weak solution u in V satisfying a(u,v) = F(v) for all test functions v.",
            query: "unique weak solution for coercive continuous bilinear form on Hilbert space",
        },
        EvalCorpusItem {
            title: "ergodic Theorem of Birkhoff for Measure-Preserving Systems",
            text: "For an integrable observable f on a probability space with measure-preserving transformation T, time averages (1/N) sum_{k=0}^{N-1} f(T^k x) converge almost everywhere and in L^1 to conditional expectation.",
            query: "almost everywhere convergence of time averages under measure preserving map",
        },
    ]
}

fn eval_hybrid(
    profile_str: &str,
    models_dir_opt: Option<PathBuf>,
    ort_lib_opt: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let profile = ModelProfile::from_str_lossy(profile_str);
    let paths = AppPaths::from_data_dir(AppPaths::default_app_dir());
    let models_dir = models_dir_opt.unwrap_or(paths.models_dir);
    let ort_lib = resolved_onnxruntime_path(ort_lib_opt.as_deref());
    let ready = profile_is_ready(&models_dir, profile) && ort_lib.is_some();

    println!(
        "=== Papero EvalHybrid (profile={}, models_ready={}, ort={}) ===",
        profile.as_str(),
        profile_is_ready(&models_dir, profile),
        ort_lib
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "MISSING".into())
    );

    let tmp = tempfile::tempdir()?;
    let db = Database::open(&tmp.path().join("eval.db"), 2)?;
    let bm25 = Bm25Index::open_or_create(&tmp.path().join("tantivy"))?;
    let vindex = VectorIndex::open(&tmp.path().join("vec.usearch"), VectorIndex::dimensions_for_profile(profile))?;

    let root_dir = tmp.path().join("lib");
    fs::create_dir_all(&root_dir)?;
    let root_id = new_root_id();
    let corpus = math_eval_corpus();

    let mut target_doc_ids = Vec::with_capacity(corpus.len());
    let mut passage_rows: Vec<(u64, String)> = Vec::new();
    let mut bm25_batch = Vec::new();

    {
        let w = db.writer();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: root_id.clone(),
                label: "Eval".into(),
                root_path: root_dir.to_string_lossy().into_owned(),
                is_network_or_cloud: false,
            },
        )?;
        for (i, item) in corpus.iter().enumerate() {
            let doc_id = create_heuristic_document(&w, item.title)?;
            target_doc_ids.push(doc_id);
            let file_id = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: doc_id,
                    root_id: &root_id,
                    rel_path: &format!("doc_{i:03}.pdf"),
                    format: "pdf",
                    size_bytes: 1024,
                    mtime_ns: 0,
                    blake3_hash: &format!("hash_{i}"),
                    index_state: "bm25_done",
                },
            )?;
            let pids = replace_file_pages_and_passages(
                &w,
                file_id,
                1,
                true,
                None,
                &[FilePageInsert {
                    file_id,
                    page_ord: 1,
                    printed_label: Some("1".into()),
                    width_pts: 612.0,
                    height_pts: 792.0,
                    text_source: "eval".into(),
                }],
                &[PassageInsert {
                    document_id: doc_id,
                    file_id,
                    page_ord: 1,
                    printed_label: Some("1".into()),
                    chunk_index: 0,
                    is_alt_formula: false,
                    token_count: 45,
                    text_content: item.text.to_string(),
                    bboxes_blob: Vec::new(),
                }],
            )?;
            passage_rows.push((pids[0], item.text.to_string()));
            bm25_batch.push(Bm25Passage {
                passage_id: pids[0],
                document_id: doc_id as u64,
                file_id: file_id as u64,
                root_id: root_id.clone(),
                text_content: item.text.to_string(),
            });
        }
        // Add 200 distractor math passages to test discrimination
        for j in 0..200 {
            let doc_id = create_heuristic_document(&w, &format!("Mathematical Notes #{j}"))?;
            let file_id = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: doc_id,
                    root_id: &root_id,
                    rel_path: &format!("distractor_{j:03}.pdf"),
                    format: "pdf",
                    size_bytes: 1024,
                    mtime_ns: 0,
                    blake3_hash: &format!("dhash_{j}"),
                    index_state: "bm25_done",
                },
            )?;
            let txt = format!(
                "Section {j}: Algebraic properties of commutative rings, ideals, localizations, polynomial quotients, and module homomorphisms in characteristic {p}.",
                p = (j % 17) + 2
            );
            let pids = replace_file_pages_and_passages(
                &w,
                file_id,
                1,
                true,
                None,
                &[FilePageInsert {
                    file_id,
                    page_ord: 1,
                    printed_label: Some("1".into()),
                    width_pts: 612.0,
                    height_pts: 792.0,
                    text_source: "eval".into(),
                }],
                &[PassageInsert {
                    document_id: doc_id,
                    file_id,
                    page_ord: 1,
                    printed_label: Some("1".into()),
                    chunk_index: 0,
                    is_alt_formula: false,
                    token_count: 25,
                    text_content: txt.clone(),
                    bboxes_blob: Vec::new(),
                }],
            )?;
            passage_rows.push((pids[0], txt.clone()));
            bm25_batch.push(Bm25Passage {
                passage_id: pids[0],
                document_id: doc_id as u64,
                file_id: file_id as u64,
                root_id: root_id.clone(),
                text_content: txt,
            });
        }
    }
    bm25.add_passages(&bm25_batch)?;
    bm25.commit()?;

    let embedder_slot = LazyModelSlot::<OnnxEmbedder>::new();
    let reranker_slot = LazyModelSlot::<OnnxReranker>::new();

    if ready {
        println!("Embedding {} evaluation passages with real ONNX runtime...", passage_rows.len());
        let t_emb = Instant::now();
        for chunk in passage_rows.chunks(16) {
            let texts: Vec<&str> = chunk.iter().map(|(_, s)| s.as_str()).collect();
            let vecs = embedder_slot.with_model(
                || OnnxEmbedder::load(&models_dir, profile, ort_lib.as_deref(), 4),
                |emb| emb.embed_passages(&texts),
            )?;
            let pairs: Vec<(u64, Vec<f32>)> = chunk
                .iter()
                .zip(vecs)
                .map(|((pid, _), v)| (*pid, v))
                .collect();
            vindex.upsert_batch(&pairs)?;
        }
        println!("Embedded {} passages in {:.2} s", passage_rows.len(), t_emb.elapsed().as_secs_f64());
    }

    // Compare modes: BM25-only vs BM25+Vector vs BM25+Vector+Reranker
    evaluate_mode("BM25-only", &db, &bm25, None, &embedder_slot, &reranker_slot, &models_dir, profile, ort_lib.as_deref(), false, &corpus, &target_doc_ids)?;
    if ready {
        evaluate_mode("BM25+Vector (RRF)", &db, &bm25, Some(&vindex), &embedder_slot, &reranker_slot, &models_dir, profile, ort_lib.as_deref(), false, &corpus, &target_doc_ids)?;
        evaluate_mode("BM25+Vector+Rerank", &db, &bm25, Some(&vindex), &embedder_slot, &reranker_slot, &models_dir, profile, ort_lib.as_deref(), true, &corpus, &target_doc_ids)?;
    } else {
        println!("NOTE: Skipping ONNX vector & reranker eval because ONNX models or runtime are not installed yet.");
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn evaluate_mode(
    label: &str,
    db: &Database,
    bm25: &Bm25Index,
    vindex: Option<&VectorIndex>,
    embedder_slot: &LazyModelSlot<OnnxEmbedder>,
    reranker_slot: &LazyModelSlot<OnnxReranker>,
    models_dir: &std::path::Path,
    profile: ModelProfile,
    ort_lib: Option<&std::path::Path>,
    rerank_enabled: bool,
    corpus: &[EvalCorpusItem],
    target_doc_ids: &[i64],
) -> Result<(), Box<dyn std::error::Error>> {
    let opts = HybridSearchOptions {
        top_n_per_branch: 50,
        rerank_enabled,
        rerank_top_m: match profile {
            ModelProfile::Light => 25,
            ModelProfile::Quality => 12,
        },
        top_k_docs: 30,
        min_score: 0.0,
        rrf_k: 60.0,
        aggregation: DocScoreAggregation::Max,
        work_category: None,
        filter: Default::default(),
    };

    let mut r5 = 0usize;
    let mut r10 = 0usize;
    let mut r20 = 0usize;
    let mut mrr_sum = 0.0f64;
    let mut latencies_ms = Vec::with_capacity(corpus.len());

    let reader = db.reader();
    for (i, item) in corpus.iter().enumerate() {
        let target_doc = target_doc_ids[i];
        let ctx = vindex.map(|vi| HybridSearchContext {
            vector_index: Some(vi),
            embedder_slot: Some(embedder_slot),
            embedder_loader: Some(|| OnnxEmbedder::load(models_dir, profile, ort_lib, 4)),
            reranker_slot: if rerank_enabled { Some(reranker_slot) } else { None },
            reranker_loader: Some(|| OnnxReranker::load(models_dir, profile, ort_lib, 4)),
        });
        let resp = hybrid_search(&reader, bm25, item.query, None, &opts, ctx)?;
        latencies_ms.push(resp.stats.total_ms);

        if let Some(pos) = resp.results.iter().position(|d| d.document_id == target_doc) {
            let rank = pos + 1;
            if rank <= 5 { r5 += 1; }
            if rank <= 10 { r10 += 1; }
            if rank <= 20 { r20 += 1; }
            mrr_sum += 1.0 / (rank as f64);
        }
    }

    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = corpus.len() as f64;
    let median_ms = latencies_ms[latencies_ms.len() / 2];
    println!(
        "[{label}] Recall@5={:.1}%, Recall@10={:.1}%, Recall@20={:.1}%, MRR={:.3}, median={:.1} ms",
        100.0 * (r5 as f64) / n,
        100.0 * (r10 as f64) / n,
        100.0 * (r20 as f64) / n,
        mrr_sum / n,
        median_ms
    );
    Ok(())
}

fn bench_search(
    count: usize,
    profile_str: &str,
    models_dir_opt: Option<PathBuf>,
    ort_lib_opt: Option<PathBuf>,
    queries: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let profile = ModelProfile::from_str_lossy(profile_str);
    let paths = AppPaths::from_data_dir(AppPaths::default_app_dir());
    let models_dir = models_dir_opt.unwrap_or(paths.models_dir);
    let ort_lib = resolved_onnxruntime_path(ort_lib_opt.as_deref());
    let models_ready = profile_is_ready(&models_dir, profile) && ort_lib.is_some();

    let rss_initial_mb = current_process_rss_bytes().unwrap_or(0) as f64 / (1024.0 * 1024.0);
    println!("=== Papero BenchSearch ({count} docs, profile={}, models_ready={models_ready}) ===", profile.as_str());
    println!(
        "Idle RSS BEFORE loading ONNX models: {:.1} MB (target <= 250.0 MB -> {})",
        rss_initial_mb,
        if rss_initial_mb <= 250.0 { "PASS" } else { "FAIL" }
    );

    let tmp = tempfile::tempdir()?;
    let db = Database::open(&tmp.path().join("bench.db"), 4)?;
    let bm25 = Bm25Index::open_or_create(&tmp.path().join("tantivy"))?;
    let dim = VectorIndex::dimensions_for_profile(profile);
    let vindex = VectorIndex::open(&tmp.path().join("bench.usearch"), dim)?;

    let lib_dir = tmp.path().join("lib");
    fs::create_dir_all(&lib_dir)?;
    let root_id = new_root_id();
    let corpus = math_eval_corpus();

    let setup_t0 = Instant::now();
    let mut seed_passages = Vec::with_capacity(corpus.len());
    let mut all_pids = Vec::with_capacity(count);
    let mut bm25_batch = Vec::with_capacity(count);

    {
        let w = db.writer();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: root_id.clone(),
                label: "Bench".into(),
                root_path: lib_dir.to_string_lossy().into_owned(),
                is_network_or_cloud: false,
            },
        )?;

        for i in 0..count {
            let item = &corpus[i % corpus.len()];
            let is_seed = i < corpus.len();
            let text = if is_seed {
                item.text.to_string()
            } else {
                format!(
                    "Document #{i} on mathematical analysis, Hilbert operators, topological manifolds, measure theory, and algebraic extensions (note series {}).",
                    i % 97
                )
            };
            let title = if is_seed {
                item.title.to_string()
            } else {
                format!("Math Monograph Volume {i:04}")
            };
            let doc_id = create_heuristic_document(&w, &title)?;
            let file_id = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: doc_id,
                    root_id: &root_id,
                    rel_path: &format!("shelf_{:02}/doc_{i:04}.pdf", i % 50),
                    format: "pdf",
                    size_bytes: 2048,
                    mtime_ns: 0,
                    blake3_hash: &format!("bh_{i:04}"),
                    index_state: "ready",
                },
            )?;
            let pids = replace_file_pages_and_passages(
                &w,
                file_id,
                1,
                true,
                None,
                &[FilePageInsert {
                    file_id,
                    page_ord: 1,
                    printed_label: Some("1".into()),
                    width_pts: 612.0,
                    height_pts: 792.0,
                    text_source: "bench".into(),
                }],
                &[PassageInsert {
                    document_id: doc_id,
                    file_id,
                    page_ord: 1,
                    printed_label: Some("1".into()),
                    chunk_index: 0,
                    is_alt_formula: false,
                    token_count: 32,
                    text_content: text.clone(),
                    bboxes_blob: Vec::new(),
                }],
            )?;
            if is_seed {
                seed_passages.push((pids[0], text.clone()));
            }
            all_pids.push(pids[0]);
            bm25_batch.push(Bm25Passage {
                passage_id: pids[0],
                document_id: doc_id as u64,
                file_id: file_id as u64,
                root_id: root_id.clone(),
                text_content: text,
            });
        }
    }
    bm25.add_passages(&bm25_batch)?;
    bm25.commit()?;

    let embedder_slot = LazyModelSlot::<OnnxEmbedder>::new();
    let reranker_slot = LazyModelSlot::<OnnxReranker>::new();

    if models_ready {
        // Embed the seed domain passages with real ONNX and populate 5,000-doc HNSW graph
        // with deterministic normalized vectors so HNSW traversal & memory footprint match 5,000 docs.
        let seed_texts: Vec<&str> = seed_passages.iter().map(|(_, t)| t.as_str()).collect();
        let seed_vecs = embedder_slot.with_model(
            || OnnxEmbedder::load_isolated(&models_dir, profile, ort_lib.as_deref(), 4),
            |emb| emb.embed_passages(&seed_texts),
        )?;
        let mut vec_items: Vec<(u64, Vec<f32>)> = Vec::with_capacity(count);
        for (i, &pid) in all_pids.iter().enumerate() {
            if i < seed_vecs.len() {
                vec_items.push((pid, seed_vecs[i].clone()));
            } else {
                let mut v = vec![0.0_f32; dim];
                let a = (i * 13 + 5) % dim;
                let b = (i * 29 + 11) % dim;
                v[a] = 0.8;
                v[b] = 0.6;
                vec_items.push((pid, v));
            }
        }
        vindex.upsert_batch(&vec_items)?;
    }

    let rss_after_corpus_mb = current_process_rss_bytes().unwrap_or(0) as f64 / (1024.0 * 1024.0);
    println!(
        "Corpus ready ({count} docs, {} vector keys) in {:.2} s (footprint={:.1} MB)",
        vindex.len(),
        setup_t0.elapsed().as_secs_f64(),
        rss_after_corpus_mb
    );

    let reader = db.reader();
    let run_count = queries.max(1);

    // 1) Content search WITHOUT reranking
    // Warmup 1 query so lazy model load isn't counted as steady-state search latency, OR measure warm + cold separately!
    let warmup_opts = HybridSearchOptions {
        rerank_enabled: false,
        ..Default::default()
    };
    let _ = hybrid_search(
        &reader,
        &bm25,
        corpus[0].query,
        None,
        &warmup_opts,
        if models_ready {
            Some(HybridSearchContext {
                vector_index: Some(&vindex),
                embedder_slot: Some(&embedder_slot),
                embedder_loader: Some(|| {
                    OnnxEmbedder::load_isolated(&models_dir, profile, ort_lib.as_deref(), 4)
                }),
                reranker_slot: None,
                reranker_loader: None::<fn() -> papero_core::error::Result<OnnxReranker>>,
            })
        } else {
            None
        },
    )?;

    let mut no_rerank_ms = Vec::with_capacity(run_count);
    for q_idx in 0..run_count {
        let q = corpus[q_idx % corpus.len()].query;
        let resp = hybrid_search(
            &reader,
            &bm25,
            q,
            None,
            &warmup_opts,
            if models_ready {
                Some(HybridSearchContext {
                    vector_index: Some(&vindex),
                    embedder_slot: Some(&embedder_slot),
                    embedder_loader: Some(|| {
                        OnnxEmbedder::load_isolated(&models_dir, profile, ort_lib.as_deref(), 4)
                    }),
                    reranker_slot: None,
                    reranker_loader: None::<fn() -> papero_core::error::Result<OnnxReranker>>,
                })
            } else {
                None
            },
        )?;
        no_rerank_ms.push(resp.stats.total_ms);
    }
    no_rerank_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let no_rerank_med_s = no_rerank_ms[no_rerank_ms.len() / 2] / 1000.0;
    let no_rerank_max_s = no_rerank_ms.last().copied().unwrap_or(0.0) / 1000.0;
    println!(
        "Content search WITHOUT rerank ({count} docs): median={:.4} s, max={:.4} s (target <= 0.500 s -> {})",
        no_rerank_med_s,
        no_rerank_max_s,
        if no_rerank_med_s <= 0.5 { "PASS" } else { "FAIL" }
    );

    // 2) Content search WITH reranking
    if models_ready {
        let rerank_opts = HybridSearchOptions {
            rerank_enabled: true,
            rerank_top_m: match profile {
                ModelProfile::Light => 25,
                ModelProfile::Quality => 12,
            },
            ..Default::default()
        };
        // Warmup cross-encoder session once
        let _ = hybrid_search(
            &reader,
            &bm25,
            corpus[0].query,
            None,
            &rerank_opts,
            Some(HybridSearchContext {
                vector_index: Some(&vindex),
                embedder_slot: Some(&embedder_slot),
                embedder_loader: Some(|| {
                    OnnxEmbedder::load_isolated(&models_dir, profile, ort_lib.as_deref(), 4)
                }),
                reranker_slot: Some(&reranker_slot),
                reranker_loader: Some(|| {
                    OnnxReranker::load_isolated(&models_dir, profile, ort_lib.as_deref(), 4)
                }),
            }),
        )?;

        let mut rerank_ms = Vec::with_capacity(run_count);
        for q_idx in 0..run_count {
            let q = corpus[q_idx % corpus.len()].query;
            let resp = hybrid_search(
                &reader,
                &bm25,
                q,
                None,
                &rerank_opts,
                Some(HybridSearchContext {
                    vector_index: Some(&vindex),
                    embedder_slot: Some(&embedder_slot),
                    embedder_loader: Some(|| {
                        OnnxEmbedder::load_isolated(&models_dir, profile, ort_lib.as_deref(), 4)
                    }),
                    reranker_slot: Some(&reranker_slot),
                    reranker_loader: Some(|| {
                        OnnxReranker::load_isolated(&models_dir, profile, ort_lib.as_deref(), 4)
                    }),
                }),
            )?;
            rerank_ms.push(resp.stats.total_ms);
        }
        rerank_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let rerank_med_s = rerank_ms[rerank_ms.len() / 2] / 1000.0;
        let rerank_max_s = rerank_ms.last().copied().unwrap_or(0.0) / 1000.0;
        println!(
            "Content search WITH rerank ({count} docs, M={}): median={:.4} s, max={:.4} s (target <= 1.500 s -> {})",
            rerank_opts.rerank_top_m,
            rerank_med_s,
            rerank_max_s,
            if rerank_med_s <= 1.5 { "PASS" } else { "FAIL" }
        );
    }

    let rss_loaded_mb = current_process_rss_bytes().unwrap_or(0) as f64 / (1024.0 * 1024.0);
    embedder_slot.unload();
    reranker_slot.unload();
    let rss_unloaded_mb = current_process_rss_bytes().unwrap_or(0) as f64 / (1024.0 * 1024.0);
    println!(
        "Main app RSS while worker models loaded: {:.1} MB | Idle RSS AFTER worker unload: {:.1} MB (target <= 250.0 MB -> {})",
        rss_loaded_mb,
        rss_unloaded_mb,
        if rss_unloaded_mb <= 250.0 { "PASS" } else { "FAIL" }
    );

    Ok(())
}
