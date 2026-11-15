# Third-Party Licenses & Attribution Notice

## Papero Application License

Papero's original source code (the Rust crates `papero-core`, `papero-app`, `papero-eval`, and the Svelte/TypeScript frontend) is dual-licensed under your choice of:

- **MIT License** ([`LICENSE-MIT`](LICENSE-MIT))
- **Apache License, Version 2.0** ([`LICENSE-APACHE`](LICENSE-APACHE))

---

## Bundled Native Libraries & Standalone Tools

| Component | Role in Papero | License | Compliance Notes |
| --- | --- | --- | --- |
| **PDFium** (`bblanchon/pdfium-binaries`, non-V8 build) | PDF text/bbox extraction, page metrics, raster rendering for OCR | `BSD-3-Clause` | Loaded dynamically at runtime via `pdfium-render`; no V8/ICU bloat included. |
| **ONNX Runtime** (CPU / CoreML build, no DirectML) | Local INT8 neural embedding & cross-encoder reranking | `MIT` | Loaded dynamically at runtime via `ort` (`load-dynamic`). |
| **DjVuLibre 3.5.30** (`ddjvu`, `djvused`, `djvutxt` + shared libs) | DjVu page rendering and hidden text/outline inspection | `GPL-2.0-or-later` | Executed **strictly as separate OS child processes** (`mere aggregation` under GPLv2 §2). Full GPLv2 text is included in [`licenses/GPL-2.0-DjVuLibre.txt`](licenses/GPL-2.0-DjVuLibre.txt) and written source offer in [`licenses/SOURCE_OFFER.txt`](licenses/SOURCE_OFFER.txt). Each GitHub Release attaches `djvulibre-3.5.30.tar.gz` (`SHA-256: ee5e457d4cfebe566f94b99e5e3d3cc7f5c79ddb741c2ac2ba2e456f00329644`). |
| **libjpeg-turbo** / **libtiff** / **zlib** (transitive runtime libs for DjVuLibre where bundled) | Raster codecs linked by `ddjvu`/`libdjvulibre` | `IJG` / `BSD-3-Clause` / `libtiff` / `Zlib` | Redistributed verbatim solely as runtime dependencies of the standalone DjVuLibre binaries. |
| **SQLite** (bundled inside `rusqlite`) | Local relational & settings storage | `Public Domain` | Embedded static library inside `rusqlite`. |
| **Tesseract OCR** *(optional, installed separately by user)* | On-demand & background OCR for scanned PDF/DjVu pages | `Apache-2.0` | Never bundled; invoked optionally as an external OS child process if installed on the user's system. |

---

## Optional Pre-Trained ML Models (Downloaded at User Request)

All ONNX INT8 models are permissive, non-commercial-restriction-free open-source models downloaded directly from Hugging Face with hardcoded SHA-256 verification:

| Profile & Role | Base Model & ONNX Artifact | License |
| --- | --- | --- |
| Light Embedding (384-d) | `intfloat/multilingual-e5-small` (`Xenova/multilingual-e5-small`) | `MIT` |
| Light Reranker | `cross-encoder/mmarco-mMiniLMv2-L12-H384-v1` (`Xenova/mmarco-mMiniLMv2-L12-H384-v1`) | `Apache-2.0` |
| Quality Embedding (1024-d) | `BAAI/bge-m3` (`Xenova/bge-m3`) | `MIT` |
| Quality Reranker | `BAAI/bge-reranker-v2-m3` (`onnx-community/bge-reranker-v2-m3-ONNX`) | `Apache-2.0` |

---

## Bibliographic Data Sources & Taxonomies

- **MSC2020 (Mathematics Subject Classification)**: Embedded local 63-top-level classification hierarchy maintained jointly by *Mathematical Reviews* (AMS) and *zbMATH Open* (FIZ Karlsruhe).
- **zbMATH Open API**: Metadata records fetched on user/bibliographic lookup carry zbMATH Open attribution (`CC-BY-SA 4.0`, FIZ Karlsruhe); BibTeX exports preserve source attribution comments when requested.
- **AMS MRLookup** (`https://mathscinet.ams.org/mrlookup`): Free public web tool; optional and toggleable (disabled by default in bulk lookup workflows if opted out), rate-limited for individual user lookups in accordance with AMS terms.
- **DBLP Computer Science Bibliography (`dblp.org`)**: Open computer science bibliographic metadata (`CC0 1.0` Public Domain Dedication, Schloss Dagstuhl – LZI).
- **OpenAlex API (`api.openalex.org`)**: Cross-disciplinary open scholarly graph metadata (`CC0 1.0`, OurResearch), supporting optional local polite-pool `mailto` parameter.
- **Crossref REST API (`api.crossref.org`)**: Bibliographic metadata (`CC0` / publisher public metadata) queried conservatively (`>= 1 req/s` adaptive polite pool when user provides an optional local contact email).
- **arXiv API (`export.arxiv.org`)**: Preprint metadata (`CC0`) queried at `<= 1 req / 3s`.
- **Semantic Scholar Academic Graph API (`api.semanticscholar.org`)**: Free unauthenticated scholarly metadata queried conservatively under Semantic Scholar API terms.
- **INSPIRE-HEP API (`inspirehep.net/api`)**: High-energy & mathematical physics open metadata (`CC0 1.0`).
- **DataCite REST API (`api.datacite.org`)**: Research datasets, software, and institutional repository DOI metadata (`CC0 1.0`).
- **Europe PMC REST API (`ebi.ac.uk/europepmc`)**: Life sciences, biostatistics, and computational biology open bibliographic metadata.
- **Open Library API (`openlibrary.org`)**: Book metadata queried strictly for books (`<= 1 req/s`, persistent local cache, 429 circuit breaker).

---

## Direct Rust Dependencies (`Cargo.toml`)

All workspace direct dependencies carry verified permissive licenses:

| Crate | Version | SPDX License |
| --- | --- | --- |
| `blake3` | `1.8.7` | `CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception` |
| `chrono` | `0.4.45` | `MIT OR Apache-2.0` |
| `clap` | `4.6.7` | `MIT OR Apache-2.0` |
| `futures-util` | `0.3.34` | `MIT OR Apache-2.0` |
| `hex` | `0.4.3` | `MIT OR Apache-2.0` |
| `http` | `1.5.0` | `MIT OR Apache-2.0` |
| `keyring` | `3.6.3` | `MIT OR Apache-2.0` |
| `ndarray` | `0.17.2` | `MIT OR Apache-2.0` |
| `notify` | `7.0.0` | `CC0-1.0` |
| `notify-debouncer-full` | `0.4.0` | `MIT OR Apache-2.0` |
| `ort` | `2.0.0-rc.15` | `MIT OR Apache-2.0` |
| `pdfium-render` | `0.8.37` | `MIT OR Apache-2.0` |
| `percent-encoding` | `2.3.2` | `MIT OR Apache-2.0` |
| `png` | `0.18.1` | `MIT OR Apache-2.0` |
| `quick-xml` | `0.42.0` | `MIT` |
| `rayon` | `1.12.0` | `MIT OR Apache-2.0` |
| `regex` | `1.13.1` | `MIT OR Apache-2.0` |
| `reqwest` | `0.13.5` | `MIT OR Apache-2.0` |
| `rusqlite` | `0.32.1` | `MIT` |
| `serde` | `1.0.229` | `MIT OR Apache-2.0` |
| `serde_json` | `1.0.154` | `MIT OR Apache-2.0` |
| `sha2` | `0.10.9` | `MIT OR Apache-2.0` |
| `strsim` | `0.11.1` | `MIT` |
| `tantivy` | `0.22.1` | `MIT` |
| `tauri` | `2.12.5` | `Apache-2.0 OR MIT` |
| `tauri-build` | `2.7.4` | `Apache-2.0 OR MIT` |
| `tauri-plugin-dialog` | `2.8.4` | `Apache-2.0 OR MIT` |
| `tempfile` | `3.27.0` | `MIT OR Apache-2.0` |
| `thiserror` | `2.0.21` | `MIT OR Apache-2.0` |
| `thread-priority` | `1.2.0` | `MIT` |
| `tokenizers` | `0.22.2` | `Apache-2.0` |
| `tokio` | `1.54.1` | `MIT` |
| `tracing` | `0.1.44` | `MIT` |
| `tracing-appender` | `0.2.5` | `MIT` |
| `tracing-subscriber` | `0.3.23` | `MIT` |
| `unicode-normalization` | `0.1.26` | `(MIT OR Apache-2.0) AND Unicode-3.0` |
| `url` | `2.5.8` | `MIT OR Apache-2.0` |
| `usearch` | `2.26.6` | `Apache-2.0` |
| `uuid` | `1.29.2` | `Apache-2.0 OR MIT` |
| `walkdir` | `2.5.0` | `Unlicense/MIT` |

---

## Frontend Dependencies (`frontend/package.json`)

| Package | Version | SPDX License |
| --- | --- | --- |
| `svelte` | `5.57.3` | `MIT` |
| `pdfjs-dist` | `4.10.38` | `Apache-2.0` |
| `@tauri-apps/api` | `2.12.4` | `Apache-2.0 OR MIT` |
| `@tauri-apps/plugin-dialog` | `2.8.4` | `MIT OR Apache-2.0` |
| `vite` / `typescript` / `vitest` (build/dev only) | `6.x` / `5.7.x` / `3.x` | `MIT` / `Apache-2.0` |

Transitive dependency licenses may also be inspected via `cargo metadata` and `npm ls` in the source repository.
