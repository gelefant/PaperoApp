use crate::state::AppState;
use http::{header, Request, Response, StatusCode};
use papero_core::db::repos::docs::get_file_location;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64, // inclusive
}

/// Parses an HTTP `Range: bytes=...` header against `total_len`.
/// Returns `Ok(None)` if no valid bytes range is specified, `Err(())` if unsatisfiable.
pub fn parse_byte_range(header_val: &str, total_len: u64) -> Result<Option<ByteRange>, ()> {
    let trimmed = header_val.trim();
    let Some(spec) = trimmed.strip_prefix("bytes=") else {
        return Ok(None);
    };
    // Only handle single range requests (standard for pdf.js range chunking)
    let first_range = spec.split(',').next().unwrap_or("").trim();
    let Some((start_s, end_s)) = first_range.split_once('-') else {
        return Err(());
    };
    if total_len == 0 {
        return Err(());
    }

    if start_s.is_empty() {
        // Suffix byte range: `-500` -> last 500 bytes
        let suffix: u64 = end_s.trim().parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let start = total_len.saturating_sub(suffix);
        return Ok(Some(ByteRange {
            start,
            end: total_len - 1,
        }));
    }

    let start: u64 = start_s.trim().parse().map_err(|_| ())?;
    if start >= total_len {
        return Err(());
    }
    let end = if end_s.trim().is_empty() {
        total_len - 1
    } else {
        let parsed_end: u64 = end_s.trim().parse().map_err(|_| ())?;
        if parsed_end < start {
            return Err(());
        }
        parsed_end.min(total_len - 1)
    };

    Ok(Some(ByteRange { start, end }))
}

pub fn handle_papero_doc_request(
    state: Arc<AppState>,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    if request.method() == http::Method::OPTIONS {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, OPTIONS")
            .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "Range, Content-Type")
            .header(
                header::ACCESS_CONTROL_EXPOSE_HEADERS,
                "Accept-Ranges, Content-Range, Content-Length",
            )
            .body(Vec::new())
            .unwrap();
    }

    let path = request.uri().path();
    let query = request.uri().query().unwrap_or("");
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    match segments.as_slice() {
        ["pdf", file_id_str] => {
            let Ok(file_id) = file_id_str.parse::<i64>() else {
                return error_response(StatusCode::BAD_REQUEST, "invalid file_id");
            };
            serve_pdf_range(&state, file_id, &request)
        }
        ["djvu-page", file_id_str, page_ord_str] => {
            let (Ok(file_id), Ok(page_ord)) =
                (file_id_str.parse::<i64>(), page_ord_str.parse::<u32>())
            else {
                return error_response(StatusCode::BAD_REQUEST, "invalid djvu page parameters");
            };
            let dpi = parse_query_u32(query, "dpi").unwrap_or(150);
            serve_djvu_page(&state, file_id, page_ord.max(1), dpi)
        }
        _ => error_response(StatusCode::NOT_FOUND, "unknown papero-doc route"),
    }
}

fn serve_pdf_range(
    state: &AppState,
    file_id: i64,
    request: &Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let loc = {
        let r = state.db.reader();
        match get_file_location(&r, file_id) {
            Ok(Some(l)) => l,
            Ok(None) => return error_response(StatusCode::NOT_FOUND, "file not found"),
            Err(e) => return error_response(StatusCode::FORBIDDEN, &e.to_string()),
        }
    };

    if !loc.is_available || !loc.abs_path.exists() {
        return error_response(StatusCode::NOT_FOUND, "source folder unreachable");
    }

    let mut file = match File::open(&loc.abs_path) {
        Ok(f) => f,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let total_len = match file.metadata() {
        Ok(m) => m.len(),
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    if let Some(range_val) = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
    {
        match parse_byte_range(range_val, total_len) {
            Ok(Some(range)) => {
                let len = (range.end - range.start + 1) as usize;
                let mut buf = vec![0u8; len];
                if let Err(e) = file
                    .seek(SeekFrom::Start(range.start))
                    .and_then(|_| file.read_exact(&mut buf))
                {
                    return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
                }
                return Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(header::CONTENT_TYPE, "application/pdf")
                    .header(header::ACCEPT_RANGES, "bytes")
                    .header(
                        header::CONTENT_RANGE,
                        format!("bytes {}-{}/{total_len}", range.start, range.end),
                    )
                    .header(header::CONTENT_LENGTH, len.to_string())
                    .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .header(
                        header::ACCESS_CONTROL_EXPOSE_HEADERS,
                        "Accept-Ranges, Content-Range, Content-Length",
                    )
                    .body(buf)
                    .unwrap();
            }
            Ok(None) => {}
            Err(()) => {
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{total_len}"))
                    .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .body(Vec::new())
                    .unwrap();
            }
        }
    }

    // Full file fallback (still advertises Accept-Ranges: bytes so pdf.js switches to Range requests)
    if request.method() == http::Method::HEAD {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/pdf")
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, total_len.to_string())
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(
                header::ACCESS_CONTROL_EXPOSE_HEADERS,
                "Accept-Ranges, Content-Range, Content-Length",
            )
            .body(Vec::new())
            .unwrap();
    }

    let mut full = Vec::with_capacity(total_len as usize);
    if let Err(e) = file.read_to_end(&mut full) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/pdf")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, full.len().to_string())
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "Accept-Ranges, Content-Range, Content-Length",
        )
        .body(full)
        .unwrap()
}

fn serve_djvu_page(
    state: &AppState,
    file_id: i64,
    page_ord: u32,
    dpi: u32,
) -> Response<Vec<u8>> {
    let loc = {
        let r = state.db.reader();
        match get_file_location(&r, file_id) {
            Ok(Some(l)) => l,
            Ok(None) => return error_response(StatusCode::NOT_FOUND, "file not found"),
            Err(e) => return error_response(StatusCode::FORBIDDEN, &e.to_string()),
        }
    };

    if !loc.is_available || !loc.abs_path.exists() {
        return error_response(StatusCode::NOT_FOUND, "source folder unreachable");
    }

    let ddjvu_bin = state
        .config
        .read()
        .expect("config rwlock poisoned")
        .ddjvu_bin
        .clone();

    match state.djvu_cache.render_page_png(
        &loc.abs_path,
        &loc.blake3_hash,
        page_ord,
        dpi,
        ddjvu_bin.as_deref(),
    ) {
        Ok(png_bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/png")
            .header(header::CONTENT_LENGTH, png_bytes.len().to_string())
            .header(header::CACHE_CONTROL, "max-age=3600")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(png_bytes)
            .unwrap(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

fn parse_query_u32(query: &str, key: &str) -> Option<u32> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return v.parse::<u32>().ok();
            }
        }
    }
    None
}

fn error_response(status: StatusCode, msg: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(msg.as_bytes().to_vec())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_byte_ranges_accurately() {
        assert_eq!(
            parse_byte_range("bytes=0-1023", 5000),
            Ok(Some(ByteRange { start: 0, end: 1023 }))
        );
        assert_eq!(
            parse_byte_range("bytes=4500-99999", 5000),
            Ok(Some(ByteRange { start: 4500, end: 4999 }))
        );
        assert_eq!(
            parse_byte_range("bytes=100-", 5000),
            Ok(Some(ByteRange { start: 100, end: 4999 }))
        );
        assert_eq!(
            parse_byte_range("bytes=-500", 5000),
            Ok(Some(ByteRange { start: 4500, end: 4999 }))
        );
        assert_eq!(parse_byte_range("bytes=6000-7000", 5000), Err(()));
        assert_eq!(parse_byte_range("bytes=300-100", 5000), Err(()));
    }

    #[test]
    fn serves_206_partial_content_for_pdf_range_requests() {
        use papero_core::config::AppPaths;
        use papero_core::db::repos::docs::{
            create_heuristic_document, insert_file_record, NewFileRecord,
        };
        use papero_core::db::repos::roots::{insert_source_root, NewSourceRoot};
        use papero_core::fs::roots::new_root_id;

        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let lib_dir = tmp.path().join("lib");
        std::fs::create_dir_all(&lib_dir).unwrap();

        let pdf_file = lib_dir.join("big.pdf");
        let payload: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&pdf_file, &payload).unwrap();

        let state = Arc::new(AppState::from_paths_without_logging(AppPaths::from_data_dir(data_dir)).unwrap());
        let root_id = new_root_id();
        let file_id = {
            let w = state.db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: root_id.clone(),
                    label: "Lib".into(),
                    root_path: lib_dir.to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();
            let doc_id = create_heuristic_document(&w, "Big Book").unwrap();
            insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: doc_id,
                    root_id: &root_id,
                    rel_path: "big.pdf",
                    format: "pdf",
                    size_bytes: payload.len() as u64,
                    mtime_ns: 0,
                    blake3_hash: "abc",
                    index_state: "ready",
                },
            )
            .unwrap()
        };

        let req = Request::builder()
            .method(http::Method::GET)
            .uri(format!("papero-doc://localhost/pdf/{file_id}"))
            .header(header::RANGE, "bytes=100-199")
            .body(Vec::new())
            .unwrap();

        let resp = handle_papero_doc_request(Arc::clone(&state), req);
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            resp.headers().get(header::CONTENT_RANGE).unwrap().to_str().unwrap(),
            "bytes 100-199/4096"
        );
        assert_eq!(resp.body(), &payload[100..200]);
    }
}
