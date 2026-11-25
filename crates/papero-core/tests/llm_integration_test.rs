use papero_core::backup::create_backup_bundle;
use papero_core::config::{AppConfig, AppPaths};
use papero_core::credentials::SecretApiKey;
use papero_core::db::repos::docs::{ContentSearchDocResult, ContentSearchPassageHit};
use papero_core::db::repos::roots::{
    insert_source_root, set_root_exclude_from_cloud_llm, NewSourceRoot,
};
use papero_core::db::Database;
use papero_core::extract::types::NormalizedRect;
use papero_core::index::bm25::Bm25Index;
use papero_core::llm::{
    accept_cloud_consent, generate_grounded_answer, generate_grounded_answer_streaming,
    list_models, prepare_document_scoped_prompt, prepare_grounded_prompt,
    send_prepared_prompt_streaming, verify_provider_key, DocLlmScope, LlmProvider, REFUSAL_IT,
};
use rusqlite::params;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
struct CapturedHttpRequest {
    method_and_path: String,
    headers_lower: String,
    body: String,
}

type MockResponse = (u16, Vec<(String, String)>, String);

async fn spawn_mock_http_server(
    responses: Vec<MockResponse>,
) -> (String, Arc<Mutex<Vec<CapturedHttpRequest>>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback mock server");
    let addr = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_bg = Arc::clone(&captured);

    tokio::spawn(async move {
        for (status, extra_headers, resp_body) in responses {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let mut buf = Vec::with_capacity(4096);
            let mut tmp = [0u8; 2048];
            let header_end = loop {
                let n = sock.read(&mut tmp).await.unwrap_or(0);
                if n == 0 {
                    break None;
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(pos + 4);
                }
            };
            let Some(hend) = header_end else {
                continue;
            };
            let header_str = String::from_utf8_lossy(&buf[..hend]).to_string();
            let content_len = header_str
                .lines()
                .find_map(|line| {
                    let (k, v) = line.split_once(':')?;
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        v.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            while buf.len() < hend + content_len {
                let n = sock.read(&mut tmp).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            let body = String::from_utf8_lossy(
                &buf[hend..buf.len().min(hend + content_len)],
            )
            .to_string();
            let first_line = header_str.lines().next().unwrap_or("").to_string();
            captured_bg.lock().unwrap().push(CapturedHttpRequest {
                method_and_path: first_line,
                headers_lower: header_str.to_ascii_lowercase(),
                body,
            });

            let mut resp = format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                resp_body.len()
            );
            for (k, v) in extra_headers {
                resp.push_str(&format!("{k}: {v}\r\n"));
            }
            resp.push_str("\r\n");
            resp.push_str(&resp_body);
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });

    (format!("http://{addr}"), captured)
}

fn setup_test_library_db(dir: &std::path::Path) -> (Database, i64, i64, i64) {
    let paths = AppPaths::from_data_dir(dir.join("appdata"));
    paths.ensure_dirs().unwrap();
    let db = Database::open(&paths.db_path, 2).unwrap();
    let w = db.writer();

    insert_source_root(
        &w,
        &NewSourceRoot {
            id: "root-open".to_string(),
            label: "Public Shelf".to_string(),
            root_path: dir.join("open").to_string_lossy().to_string(),
            is_network_or_cloud: false,
        },
    )
    .unwrap();
    insert_source_root(
        &w,
        &NewSourceRoot {
            id: "root-secret".to_string(),
            label: "Confidential Drafts".to_string(),
            root_path: dir.join("secret").to_string_lossy().to_string(),
            is_network_or_cloud: false,
        },
    )
    .unwrap();
    set_root_exclude_from_cloud_llm(&w, "root-secret", true).unwrap();

    let now = chrono::Utc::now().timestamp();
    // Doc 1: allowed
    w.execute(
        "INSERT INTO documents (id, title, title_norm, exclude_from_cloud_llm, created_at, updated_at) VALUES (10, 'Rudin Functional Analysis', 'rudin functional analysis', 0, ?1, ?1)",
        params![now],
    ).unwrap();
    w.execute(
        "INSERT INTO files (id, document_id, root_id, rel_path, format, size_bytes, mtime_ns, blake3_hash, is_available, created_at, updated_at)
         VALUES (100, 10, 'root-open', 'rudin.pdf', 'pdf', 1000, 1, 'h100', 1, ?1, ?1)",
        params![now],
    ).unwrap();

    // Doc 2: excluded directly at document level
    w.execute(
        "INSERT INTO documents (id, title, title_norm, exclude_from_cloud_llm, created_at, updated_at) VALUES (20, 'Private Unreleased Proof', 'private unreleased proof', 1, ?1, ?1)",
        params![now],
    ).unwrap();
    w.execute(
        "INSERT INTO files (id, document_id, root_id, rel_path, format, size_bytes, mtime_ns, blake3_hash, is_available, created_at, updated_at)
         VALUES (200, 20, 'root-open', 'private.pdf', 'pdf', 1000, 1, 'h200', 1, ?1, ?1)",
        params![now],
    ).unwrap();

    // Doc 3: excluded because its source folder has exclude_from_cloud_llm = 1
    w.execute(
        "INSERT INTO documents (id, title, title_norm, exclude_from_cloud_llm, created_at, updated_at) VALUES (30, 'Confidential Folder Paper', 'confidential folder paper', 0, ?1, ?1)",
        params![now],
    ).unwrap();
    w.execute(
        "INSERT INTO files (id, document_id, root_id, rel_path, format, size_bytes, mtime_ns, blake3_hash, is_available, created_at, updated_at)
         VALUES (300, 30, 'root-secret', 'folder_secret.pdf', 'pdf', 1000, 1, 'h300', 1, ?1, ?1)",
        params![now],
    ).unwrap();

    drop(w);
    (db, 10, 20, 30)
}

fn sample_results() -> Vec<ContentSearchDocResult> {
    vec![
        ContentSearchDocResult {
            document_id: 20,
            file_id: 200,
            work_type: "article".to_string(),
            title: "Private Unreleased Proof".to_string(),
            format: "pdf".to_string(),
            rel_path: "private.pdf".to_string(),
            is_available: true,
            max_score: 9.5,
            passages: vec![ContentSearchPassageHit {
                passage_id: 2001,
                file_id: 200,
                page_ord: 4,
                printed_label: None,
                score: 9.5,
                text_content: "TOP_SECRET_DOC_TOKEN_DO_NOT_SEND".to_string(),
                bboxes: vec![],
                is_alt_formula: false,
                statement_label: None,
                statement_kind: None,
            }],
        },
        ContentSearchDocResult {
            document_id: 30,
            file_id: 300,
            work_type: "article".to_string(),
            title: "Confidential Folder Paper".to_string(),
            format: "pdf".to_string(),
            rel_path: "folder_secret.pdf".to_string(),
            is_available: true,
            max_score: 8.5,
            passages: vec![ContentSearchPassageHit {
                passage_id: 3001,
                file_id: 300,
                page_ord: 7,
                printed_label: None,
                score: 8.5,
                text_content: "TOP_SECRET_FOLDER_TOKEN_DO_NOT_SEND".to_string(),
                bboxes: vec![],
                is_alt_formula: false,
                statement_label: None,
                statement_kind: None,
            }],
        },
        ContentSearchDocResult {
            document_id: 10,
            file_id: 100,
            work_type: "book".to_string(),
            title: "Rudin Functional Analysis".to_string(),
            format: "pdf".to_string(),
            rel_path: "rudin.pdf".to_string(),
            is_available: true,
            max_score: 7.2,
            passages: vec![ContentSearchPassageHit {
                passage_id: 1001,
                file_id: 100,
                page_ord: 58,
                printed_label: Some("58".to_string()),
                score: 7.2,
                text_content: "Theorem 3.2 (Hahn-Banach): Every continuous linear functional on a subspace extends norm-preservingly.".to_string(),
                bboxes: vec![NormalizedRect {
                    x: 0.12,
                    y: 0.34,
                    w: 0.72,
                    h: 0.05,
                }],
                is_alt_formula: false,
                statement_label: None,
                statement_kind: None,
            }],
        },
    ]
}

#[tokio::test]
async fn excluded_docs_and_folders_are_stripped_before_cloud_http_payload() {
    let dir = tempdir().unwrap();
    let (db, _, _, _) = setup_test_library_db(dir.path());

    let mut cfg = AppConfig {
        llm_provider: LlmProvider::Anthropic,
        llm_model: Some("claude-sonnet-4-test".to_string()),
        ..Default::default()
    };
    accept_cloud_consent(&mut cfg, LlmProvider::Anthropic);

    let mock_resp = serde_json::json!({
        "content": [{
            "type": "text",
            "text": "Hahn-Banach extends bounded linear functionals [Doc #10, p. 58]."
        }],
        "usage": { "input_tokens": 110, "output_tokens": 22 }
    })
    .to_string();

    let (base_url, captured) = spawn_mock_http_server(vec![(200, vec![], mock_resp)]).await;
    let key = SecretApiKey::new("sk-ant-never-leak-this-secret-999");

    let answer = generate_grounded_answer(
        Some(&db),
        &cfg,
        Some(&key),
        "How does Hahn-Banach extend functionals?",
        &sample_results(),
        None,
        None,
        Some(&base_url),
    )
    .await
    .unwrap();

    assert!(!answer.refused);
    assert_eq!(answer.included_passages, 1);
    assert_eq!(answer.excluded_passages, 2);
    assert_eq!(answer.usage.input_tokens, Some(110));
    assert_eq!(answer.usage.output_tokens, Some(22));

    // Citation parser maps claim back to exact (file_id, page_ord, bboxes)
    assert_eq!(answer.citations.len(), 1);
    assert_eq!(answer.citations[0].document_id, 10);
    assert_eq!(answer.citations[0].file_id, 100);
    assert_eq!(answer.citations[0].page_ord, 58);
    assert_eq!(answer.citations[0].bboxes.len(), 1);
    assert!((answer.citations[0].bboxes[0].y - 0.34).abs() < 1e-6);

    // Inspect raw HTTP request sent over the wire: ZERO excluded passages present
    {
        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].method_and_path.starts_with("POST /v1/messages"));
        assert!(reqs[0].headers_lower.contains("x-api-key: sk-ant-never-leak-this-secret-999"));
        assert!(!reqs[0].body.contains("TOP_SECRET_DOC_TOKEN_DO_NOT_SEND"));
        assert!(!reqs[0].body.contains("TOP_SECRET_FOLDER_TOKEN_DO_NOT_SEND"));
        assert!(reqs[0].body.contains("Theorem 3.2 (Hahn-Banach)"));
    }

    // Conversely, when using LOCAL Ollama, nothing is blocked by cloud exclusion rules
    cfg.llm_provider = LlmProvider::Ollama;
    let reader = db.reader();
    let local_preview = prepare_grounded_prompt(
        Some(&reader),
        &cfg,
        "test",
        &sample_results(),
        None,
        None,
    )
    .unwrap();
    assert_eq!(local_preview.preview.passages_excluded_cloud, 0);
    assert_eq!(local_preview.preview.passages_included, 3);
}

#[tokio::test]
async fn unanswerable_context_triggers_refusal_and_short_circuits_when_all_excluded() {
    let dir = tempdir().unwrap();
    let (db, _, _, _) = setup_test_library_db(dir.path());

    let mut cfg = AppConfig {
        llm_provider: LlmProvider::OpenAi,
        llm_model: Some("gpt-test".to_string()),
        ..Default::default()
    };
    accept_cloud_consent(&mut cfg, LlmProvider::OpenAi);

    let refusal_json = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": REFUSAL_IT
            }
        }],
        "usage": { "prompt_tokens": 95, "completion_tokens": 18, "total_tokens": 113 }
    })
    .to_string();

    let (base_url, captured) = spawn_mock_http_server(vec![(200, vec![], refusal_json)]).await;
    let key = SecretApiKey::new("sk-openai-secret-test-value");

    // 1) Server returns refusal phrase because passages don't answer unrelated query
    let answer = generate_grounded_answer(
        Some(&db),
        &cfg,
        Some(&key),
        "What is Fermat's Last Theorem proof by Wiles?",
        &sample_results(),
        None,
        None,
        Some(&base_url),
    )
    .await
    .unwrap();

    assert!(answer.refused);
    assert!(answer.citations.is_empty());
    assert_eq!(answer.usage.total_tokens, Some(113));
    {
        let reqs = captured.lock().unwrap();
        assert!(reqs[0].body.contains(REFUSAL_IT));
    }

    // 2) If ONLY excluded docs matched, NO HTTP call is even attempted (short-circuit offline refusal)
    let only_excluded = sample_results().into_iter().take(2).collect::<Vec<_>>();
    let offline_refusal = generate_grounded_answer(
        Some(&db),
        &cfg,
        Some(&key),
        "Anything",
        &only_excluded,
        None,
        None,
        Some(&base_url),
    )
    .await
    .unwrap();

    assert!(offline_refusal.refused);
    assert_eq!(offline_refusal.excluded_passages, 2);
    assert_eq!(offline_refusal.included_passages, 0);
    // Still only 1 HTTP call happened across both assertions!
    assert_eq!(captured.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn live_model_listing_token_usage_and_http_error_masking_for_all_providers() {
    let dir = tempdir().unwrap();
    let app_dir = dir.path().join("appdata");
    let paths = AppPaths::from_data_dir(&app_dir);
    paths.ensure_dirs().unwrap();
    let db = Database::open(&paths.db_path, 2).unwrap();
    let _bm25 = Bm25Index::open_or_create(&paths.tantivy_dir).unwrap();

    // Test Gemini list_models (via x-goog-api-key header, NOT URL query param) + chat + 429 masking
    let gemini_models = serde_json::json!({
        "models": [
            {
                "name": "models/gemini-2.5-flash",
                "displayName": "Gemini 2.5 Flash",
                "supportedGenerationMethods": ["generateContent"]
            },
            {
                "name": "models/text-embedding-004",
                "displayName": "Embedding Only",
                "supportedGenerationMethods": ["embedContent"]
            }
        ]
    })
    .to_string();

    let gemini_chat = serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{ "text": "Confirmed by theorem statement [Doc #10, p. 58]." }]
            }
        }],
        "usageMetadata": {
            "promptTokenCount": 77,
            "candidatesTokenCount": 14,
            "totalTokenCount": 91
        }
    })
    .to_string();

    let gemini_secret = "AIzaSyVerySecretGeminiKey123456";
    let err_401_echo = format!("bad credentials near {gemini_secret}");
    let (gemini_url, gemini_cap) = spawn_mock_http_server(vec![
        (200, vec![], gemini_models),
        (200, vec![], gemini_chat),
        (401, vec![], err_401_echo),
    ])
    .await;

    let key = SecretApiKey::new(gemini_secret);
    let models = list_models(LlmProvider::Gemini, Some(&key), None, Some(&gemini_url))
        .await
        .unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gemini-2.5-flash");

    // Verify Gemini header authentication & NO raw secret in URL
    {
        let reqs = gemini_cap.lock().unwrap();
        assert!(!reqs[0].method_and_path.contains(gemini_secret));
        assert!(reqs[0].headers_lower.contains(&format!("x-goog-api-key: {}", gemini_secret.to_ascii_lowercase())));
    }

    // Must refuse if first-send cloud consent not yet accepted
    let mut cfg = AppConfig {
        llm_provider: LlmProvider::Gemini,
        llm_model: Some("gemini-2.5-flash".to_string()),
        ..Default::default()
    };
    let consent_err = generate_grounded_answer(
        None,
        &cfg,
        Some(&key),
        "test",
        &sample_results()[2..],
        None,
        None,
        Some(&gemini_url),
    )
    .await
    .unwrap_err();
    assert!(consent_err.to_string().to_ascii_lowercase().contains("consent"));

    accept_cloud_consent(&mut cfg, LlmProvider::Gemini);
    let gemini_ans = generate_grounded_answer(
        None,
        &cfg,
        Some(&key),
        "test",
        &sample_results()[2..],
        None,
        None,
        Some(&gemini_url),
    )
    .await
    .unwrap();
    assert_eq!(gemini_ans.usage.input_tokens, Some(77));
    assert_eq!(gemini_ans.usage.output_tokens, Some(14));
    assert_eq!(gemini_ans.usage.total_tokens, Some(91));

    // HTTP 401 verify_provider_key MUST never leak raw key even if echoed in HTTP body
    let verify_fail = verify_provider_key(LlmProvider::Gemini, Some(&key), None, Some(&gemini_url)).await;
    assert!(!verify_fail.ok);
    assert!(!verify_fail.message.contains(gemini_secret));
    assert!(verify_fail.message.contains("401"));

    // Ollama /api/tags & /api/chat check
    let ollama_tags = serde_json::json!({
        "models": [{ "name": "qwen2.5-math:7b" }]
    })
    .to_string();
    let ollama_chat = serde_json::json!({
        "message": { "role": "assistant", "content": "See [Doc #10, p. 58]." },
        "prompt_eval_count": 40,
        "eval_count": 9
    })
    .to_string();
    let (ollama_url, _) = spawn_mock_http_server(vec![
        (200, vec![], ollama_tags),
        (200, vec![], ollama_chat),
    ])
    .await;
    let ollama_models = list_models(LlmProvider::Ollama, None, Some(&ollama_url), None)
        .await
        .unwrap();
    assert_eq!(ollama_models[0].id, "qwen2.5-math:7b");

    cfg.llm_provider = LlmProvider::Ollama;
    cfg.llm_model = Some("qwen2.5-math:7b".to_string());
    cfg.llm_ollama_url = ollama_url;
    cfg.save(&paths.config_path).unwrap();
    let ollama_ans = generate_grounded_answer(
        None,
        &cfg,
        None,
        "test",
        &sample_results()[2..],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(ollama_ans.usage.input_tokens, Some(40));
    assert_eq!(ollama_ans.usage.output_tokens, Some(9));

    // Backup inspection: verify ZERO API keys ever land inside SQLite, config.json, or .papero-backup
    let bkp_path = dir.path().join("check.papero-backup");
    create_backup_bundle(&paths, &db, &bkp_path).unwrap();
    let raw_backup = std::fs::read(&bkp_path).unwrap();
    assert!(!raw_backup
        .windows(gemini_secret.len())
        .any(|w| w == gemini_secret.as_bytes()));
    let cfg_disk = std::fs::read_to_string(&paths.config_path).unwrap();
    assert!(!cfg_disk.contains(gemini_secret));
}

#[tokio::test]
async fn streaming_deltas_and_document_scoped_qa_work_across_providers_and_enforce_privacy() {
    let dir = tempdir().unwrap();
    let (db, allowed_doc_id, excluded_doc_id, excluded_folder_doc_id) =
        setup_test_library_db(dir.path());

    // Insert indexed passage on page 58 for allowed_doc_id (file_id = 100)
    {
        let w = db.writer();
        let blob = papero_core::extract::types::pack_bboxes(&[NormalizedRect {
            x: 0.1,
            y: 0.2,
            w: 0.8,
            h: 0.05,
        }]);
        w.execute(
            "INSERT INTO passages (id, document_id, file_id, page_ord, printed_label, chunk_index, is_alt_formula, token_count, text_content, bboxes_blob)
             VALUES (9001, ?1, 100, 58, '58', 0, 0, 20, 'Theorem 3.2 (Hahn-Banach): Every continuous linear functional extends.', ?2)",
            params![allowed_doc_id, blob],
        )
        .unwrap();
        w.execute(
            "INSERT INTO passages (id, document_id, file_id, page_ord, printed_label, chunk_index, is_alt_formula, token_count, text_content, bboxes_blob)
             VALUES (9002, ?1, 200, 1, '1', 0, 0, 10, 'SECRET_DOC_PASSAGE', ?2)",
            params![excluded_doc_id, Vec::<u8>::new()],
        )
        .unwrap();
        w.execute(
            "INSERT INTO passages (id, document_id, file_id, page_ord, printed_label, chunk_index, is_alt_formula, token_count, text_content, bboxes_blob)
             VALUES (9003, ?1, 300, 1, '1', 0, 0, 10, 'SECRET_FOLDER_PASSAGE', ?2)",
            params![excluded_folder_doc_id, Vec::<u8>::new()],
        )
        .unwrap();
    }

    // 1. OpenAI SSE streaming with multi-byte UTF-8 math symbols + usage chunk
    let openai_sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"By Hahn-Banach, $\\\\forall f$, \"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"every functional extends [p. 58].\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":60,\"completion_tokens\":15,\"total_tokens\":75}}\n\n",
        "data: [DONE]\n\n"
    )
    .to_string();

    // 2. Ollama NDJSON streaming
    let ollama_ndjson = concat!(
        "{\"message\":{\"role\":\"assistant\",\"content\":\"Hahn-Banach \"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"holds [Doc #10, p. 58].\"},\"done\":true,\"prompt_eval_count\":33,\"eval_count\":7}\n"
    )
    .to_string();

    // 3. Anthropic SSE streaming
    let anthropic_sse = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":50}}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Theorem 3.2 [p. 58].\"}}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":8}}\n\n"
    )
    .to_string();

    // 4. Gemini SSE streaming
    let gemini_sse = concat!(
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Norm-preserving \"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"extension [p. 58].\"}]} }],\"usageMetadata\":{\"promptTokenCount\":41,\"candidatesTokenCount\":6,\"totalTokenCount\":47}}\n\n"
    )
    .to_string();

    let (mock_url, _cap) = spawn_mock_http_server(vec![
        (200, vec![], openai_sse),
        (200, vec![], ollama_ndjson),
        (200, vec![], anthropic_sse),
        (200, vec![], gemini_sse),
    ])
    .await;

    let mut cfg = AppConfig {
        llm_provider: LlmProvider::OpenAi,
        llm_model: Some("gpt-4o-mini".to_string()),
        ..Default::default()
    };
    accept_cloud_consent(&mut cfg, LlmProvider::OpenAi);
    accept_cloud_consent(&mut cfg, LlmProvider::Anthropic);
    accept_cloud_consent(&mut cfg, LlmProvider::Gemini);

    let key = SecretApiKey::new("sk-stream-test");

    // Document-scoped Q&A on excluded document MUST be stripped for cloud provider
    {
        let r = db.reader();
        let excl_doc_prep = prepare_document_scoped_prompt(
            &r,
            &cfg,
            excluded_doc_id,
            200,
            DocLlmScope::Page,
            1,
            None,
            "Explain",
            None,
            None,
        )
        .unwrap();
        assert_eq!(excl_doc_prep.preview.passages_included, 0);
        assert_eq!(excl_doc_prep.preview.passages_excluded_cloud, 1);

        let excl_folder_prep = prepare_document_scoped_prompt(
            &r,
            &cfg,
            excluded_folder_doc_id,
            300,
            DocLlmScope::Selection,
            1,
            Some("selected secret text"),
            "Explain selection",
            None,
            None,
        )
        .unwrap();
        assert_eq!(excl_folder_prep.preview.passages_included, 0);
        assert_eq!(excl_folder_prep.preview.passages_excluded_cloud, 1);
    }

    // OpenAI streaming on document-scoped page query resolves `[p. 58]` back to file_id=100 + bboxes!
    let prep_doc = {
        let r = db.reader();
        prepare_document_scoped_prompt(
            &r,
            &cfg,
            allowed_doc_id,
            100,
            DocLlmScope::Page,
            58,
            None,
            "What does Theorem 3.2 state?",
            None,
            None,
        )
        .unwrap()
    };
    let mut deltas = Vec::new();
    let res_openai = send_prepared_prompt_streaming(
        &cfg,
        Some(&key),
        prep_doc.clone(),
        Some(&mock_url),
        |chunk| deltas.push(chunk.to_string()),
    )
    .await
    .unwrap();
    assert_eq!(deltas.len(), 2);
    assert_eq!(res_openai.usage.total_tokens, Some(75));
    assert_eq!(res_openai.citations.len(), 1);
    assert_eq!(res_openai.citations[0].document_id, allowed_doc_id);
    assert_eq!(res_openai.citations[0].file_id, 100);
    assert_eq!(res_openai.citations[0].page_ord, 58);
    assert_eq!(res_openai.citations[0].bboxes.len(), 1);

    // Ollama streaming via `generate_grounded_answer_streaming`
    cfg.llm_provider = LlmProvider::Ollama;
    cfg.llm_ollama_url = mock_url.clone();
    let mut ollama_chunks = Vec::new();
    let res_ollama = generate_grounded_answer_streaming(
        Some(&db),
        &cfg,
        None,
        "Hahn-Banach",
        &sample_results()[2..],
        None,
        None,
        None,
        |c| ollama_chunks.push(c.to_string()),
    )
    .await
    .unwrap();
    assert_eq!(ollama_chunks.len(), 2);
    assert_eq!(res_ollama.usage.total_tokens, Some(40));
    assert_eq!(res_ollama.citations.len(), 1);

    // Anthropic streaming
    cfg.llm_provider = LlmProvider::Anthropic;
    let mut ant_chunks = Vec::new();
    let res_ant = send_prepared_prompt_streaming(
        &cfg,
        Some(&key),
        prep_doc.clone(),
        Some(&mock_url),
        |c| ant_chunks.push(c.to_string()),
    )
    .await
    .unwrap();
    assert_eq!(ant_chunks, vec!["Theorem 3.2 [p. 58]."]);
    assert_eq!(res_ant.usage.input_tokens, Some(50));
    assert_eq!(res_ant.usage.output_tokens, Some(8));
    assert_eq!(res_ant.usage.total_tokens, Some(58));
    assert_eq!(res_ant.citations[0].page_ord, 58);

    // Gemini streaming
    cfg.llm_provider = LlmProvider::Gemini;
    let mut gem_chunks = Vec::new();
    let res_gem = send_prepared_prompt_streaming(
        &cfg,
        Some(&key),
        prep_doc,
        Some(&mock_url),
        |c| gem_chunks.push(c.to_string()),
    )
    .await
    .unwrap();
    assert_eq!(gem_chunks.len(), 2);
    assert_eq!(res_gem.usage.total_tokens, Some(47));
    assert_eq!(res_gem.citations[0].page_ord, 58);
}
