use papero_core::db::Database;
use papero_core::metadata::limiter::OpenLibraryLimiter;
use papero_core::metadata::models::ExtractedIdentifiers;
use papero_core::metadata::openlibrary::{MockHttpTransport, OpenLibraryClient};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn make_mock_transport(status_code: u16) -> (MockHttpTransport, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_cb = Arc::clone(&hits);
    let transport: MockHttpTransport = Arc::new(move |_url| {
        let cnt = hits_cb.fetch_add(1, Ordering::SeqCst) + 1;
        if status_code == 200 {
            (
                200,
                format!(
                    r#"{{"title":"Algebraic Geometry #{cnt}","publishers":["Springer"],"publish_date":"1977","number_of_pages":496,"key":"/books/OL{cnt}M"}}"#
                ),
            )
        } else {
            (status_code, r#"{"error":"rate_limited"}"#.to_string())
        }
    });
    (transport, hits)
}

#[tokio::test]
async fn openlibrary_enforces_1_req_per_sec_sequentially_and_uses_permanent_cache() {
    let dir = tempdir().unwrap();
    let db = Arc::new(Database::open(&dir.path().join("papero.db"), 2).unwrap());
    let (transport, hits) = make_mock_transport(200);

    let limiter = Arc::new(OpenLibraryLimiter::default_tos());
    let client = Arc::new(
        OpenLibraryClient::new("0.3.0", None)
            .unwrap()
            .with_base_and_limiter("https://openlibrary.example", limiter)
            .with_mock_transport(transport),
    );

    let book1 = ExtractedIdentifiers {
        isbn: Some("9780387902449".into()),
        looks_like_book: true,
        ..Default::default()
    };
    let book2 = ExtractedIdentifiers {
        isbn: Some("9780521795401".into()),
        looks_like_book: true,
        ..Default::default()
    };

    // Launch two different books concurrently; sequential mutex + >= 1 req/s MUST space them >= 950 ms!
    let t0 = Instant::now();
    let (r1, r2) = tokio::join!(
        client.lookup_book(&db, &book1),
        client.lookup_book(&db, &book2)
    );
    let elapsed = t0.elapsed();
    assert_eq!(r1.unwrap().len(), 1);
    assert_eq!(r2.unwrap().len(), 1);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert!(
        elapsed >= Duration::from_millis(950),
        "expected strict >= 1 req/s pacing across 2 uncached books, got {:?}",
        elapsed
    );

    // Repeat lookup for book1 MUST hit permanent SQLite cache with ZERO transport calls!
    let r1_cached = client.lookup_book(&db, &book1).await.unwrap();
    assert_eq!(r1_cached.len(), 1);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "cached repeat book must not hit network"
    );
}

#[tokio::test]
async fn openlibrary_429_triggers_backoff_and_circuit_breaker() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("papero.db"), 2).unwrap();
    let (transport, hits) = make_mock_transport(429);

    let limiter = Arc::new(OpenLibraryLimiter::new(
        Duration::from_millis(20),
        Duration::from_secs(60),
    ));
    let client = OpenLibraryClient::new("0.3.0", None)
        .unwrap()
        .with_base_and_limiter("https://openlibrary.example", Arc::clone(&limiter))
        .with_mock_transport(transport);

    for idx in 1..=3 {
        let book = ExtractedIdentifiers {
            isbn: Some(format!("97800000000{idx}0")),
            looks_like_book: true,
            ..Default::default()
        };
        assert!(client.lookup_book(&db, &book).await.is_err());
    }

    assert_eq!(hits.load(Ordering::SeqCst), 3);
    assert!(
        limiter.is_suspended(),
        "3 consecutive 429s must open circuit breaker"
    );

    // Next call must abort locally without touching Open Library
    let book4 = ExtractedIdentifiers {
        isbn: Some("9780000000040".into()),
        looks_like_book: true,
        ..Default::default()
    };
    assert!(client.lookup_book(&db, &book4).await.is_err());
    assert_eq!(hits.load(Ordering::SeqCst), 3);
}
