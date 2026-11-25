# Papero

**A fast, free desktop library for your PDF and DjVu books and research papers.**

Papero scans your existing folders, lets you search inside all your documents in milliseconds, automatically fetches clean bibliographic metadata & BibTeX/BibLaTeX, and opens huge 800-page books without slowing down your computer.

> **Your files are always safe:** Papero treats your folders as **strictly read-only**. It never moves, renames, edits, or deletes your PDF/DjVu files. Every note, highlight, index, cache, and setting lives exclusively in local AppData (and is preserved in portable compressed backups).

---

## What Papero Does

- **One library for all your folders:** Point Papero to one or more folders on your PC/Mac/external drives — it catalogs your PDFs and DjVu files automatically, with real-time folder watching and seamless cross-OS folder rebinding.
- **Instant built-in reader:** Opens 800+ page PDFs and DjVu books right inside the app with streaming range loading, clickable internal PDF links (`\ref`, `\cite`, TOC) with **live hover micro-previews** + `Alt+←` / `Alt+→` jump history, persistent reading position, **Seppia / Night** modes, **2-page spread**, **Zen reading mode (`⌘\`)**, **side-by-side split view** of two open documents, per-file **±90° rotation**, and real `⌘F` / `Ctrl+F` visual search highlighting with hyphenation & mathematical diacritic folding.
- **Non-destructive reading annotations:** Highlight passages in 4 colors, drop page bookmarks, and write personal **Markdown / LaTeX** notes keyed by file content hash (surviving file moves/renames), searchable via the dedicated **Notes** search toggle (shown automatically once you have notes) and exportable to `.md`.
- **Smart filtered search & mathematical statements:** Find papers by title, author, year, tag, cluster, collection, folder, or full-text hybrid search (BM25 + optional local semantic vectors & cross-encoder reranker) with a dedicated **Statements only** filter (*Theorems, Lemmas, Propositions, Corollaries, Definitions*) — tolerant to accents and transliterations (`Poincaré` / `Poincare`, `Erdős` / `Erdos`, `Čech` / `Cech`).
- **Automatic citations, Smart Paste & Live `.bib` Auto-Sync (Global & per Collection):** Automatically fills bibliographic metadata from **14 free, zero-API-key databases**, supports instant *Paste DOI / arXiv / MR / ISBN / URL / BibTeX* resolution, configurable citation keys (`MR`, `AuthorYear`, `AuthorYearWord`, BibTeX/BibLaTeX) with collision disambiguation, one-click `\cite{key}` copy, and optional live auto-sync to a global `.bib` file and/or dedicated **per-collection `.bib` files** for individual papers/projects.
- **Organize, discover & stay fast:** Global **Command Palette (`⌘K`)**, multi-select batch actions (`Shift` / `⌘` / `Ctrl+Click`), right-click *Add to collection*, reorderable **Collections**, **Quick Views** (*Recent*, *Reading*, *Starred ★*) & **Saved Searches**, **Similar documents** (local HNSW), local **Cites / Cited by** links, **Suggested Classics** modal (most-cited missing papers & books from local citation caches), arXiv preprint published-version alerts, 4-level duplicate detection, MSC2020 navigation, one-file Zstd backups, local storage maintenance, and discrete GitHub release update notifications.
- **100% local and free:** Everything works offline after setup (except optional online metadata lookups or optional on-demand LLM answers), with no accounts or subscriptions required.

---

## How to Install (No Coding Required)

### 1. Open the Releases page on GitHub

Go directly to:
**<https://github.com/gelefant/PaperoApp/releases>**

*(Or from the main GitHub page of this repository, look in the **right sidebar** under **About**, click **Releases**, click the latest version at the top, and scroll down to **Assets**.)*

### 2. Download and run the installer for your system

#### Windows (10 / 11)
1. Under **Assets**, download the **`.exe`** installer (recommended, e.g. `Papero_..._x64-setup.exe`; `.msi` is also available).
2. Double-click the downloaded file to install.
3. Because Papero is free open-source software without a paid developer certificate, Windows Defender SmartScreen will show *"Windows protected your PC"*:
   - Click **More info** (*Ulteriori informazioni*)
   - Click **Run anyway** (*Esegui comunque*)

#### macOS
1. Under **Assets**, download the **`.dmg`** matching your Mac:
   - **Apple Silicon (M1 / M2 / M3 / M4 / M5):** `..._aarch64.dmg`
   - **Older Intel Macs:** `..._x64.dmg`
2. Open the `.dmg` and drag **Papero** into **Applications**.
3. Because Papero is not notarized with a paid Apple Developer account, on **macOS Sequoia (15)+**:
   - Double-click **Papero** once and dismiss the Apple warning dialog.
   - Open **System Settings → Privacy & Security**, scroll down, click **Open Anyway**, and confirm with your password.
   - *(Optional Terminal shortcut before first launch: `xattr -dr com.apple.quarantine /Applications/Papero.app`)*

#### Linux (Ubuntu / Debian)
- **`.deb` package** *(smallest download)*:
  ```bash
  sudo apt install ./Papero_*.deb
  ```
- **Portable `.AppImage`**:
  ```bash
  chmod +x Papero_*.AppImage
  ./Papero_*.AppImage
  ```

### 3. First launch
Open Papero, follow the quick 4-step welcome wizard (or restore directly from an existing `.papero-backup`), choose the folder(s) containing your PDFs/DjVu files, and let Papero index them.

*(Optional extras: if you have scanned image-only PDFs/DjVus and want Papero to read their text, you can also install **Tesseract OCR** — see below. Everything else works right out of the box.)*

---

## Technical Reference & Advanced Details

### Core Architecture & Features

- **Strictly Read-Only Guarantee & Derived Storage:** Papero never writes into configured `source_roots` — enforced across rescans, OCR, BibTeX/CSV/Markdown exports, and global/per-collection Live `.bib` Auto-Sync. All derived state (SQLite WAL schema v8, Tantivy BM25 index, disk-mmapped USearch HNSW vectors, DjVu raster LRU cache, and HTTP metadata cache) lives strictly inside the OS application data directory.
- **Incremental BLAKE3 Rescan & Source Archive:** Tracks 7 file states (`unchanged`, `new`, `copy`, `moved`, `modified`, `removed` with BLAKE3-keyed manual metadata & reading-status archive restoration, `offline`/unmounted drives preserved without purging metadata), with cross-OS folder rebinding (`rebind_source_root`).
- **Instant Unified PDF & DjVu Viewer:**
  - Byte-range streaming (`read_pdf_range_cmd` / `papero-doc://` HTTP `206 Partial Content`) opens 800+ page PDFs via lazily loaded `pdf.js` without loading whole files into RAM.
  - Background `ddjvu` rasterization with bounded disk LRU cache + invisible selectable text layer from `djvused` S-expressions.
  - Lightweight internal-only PDF link hotspots (`\ref`, `\cite`, equations, TOC) with **cropped hover micro-previews** (debounced + LRU cached) + per-tab Back/Forward jump history (`Alt+←` / `Alt+→`).
  - Non-destructive highlights (`NormalizedRect[]` in 4 colors), page bookmarks, and Markdown/LaTeX notes persisted in SQLite keyed by `blake3_hash` + `page_ord`, searchable via a conditional **Notes** search toggle and exportable to Markdown.
  - Persistent per-file reading state (`last_page_ord`, `last_opened_at`, `zoom_mode`, `custom_zoom`, `rotation_deg`), **Normal / Sepia / Night** page color filters, virtualized **2-page spread**, **Zen mode (`⌘\`)**, **side-by-side 2-document split view**, and real `⌘F` / `Ctrl+F` visual match highlighting (diacritic folding + hyphenated line-break joining across PDF & DjVu).
  - Optional detached viewer windows and OS external reader presets (`DjView`, `Skim`, `SumatraPDF`, `Okular`, `Evince`, `Zathura`, macOS `Preview`).
- **Filtered Hybrid Search, Math Statements & Lazy KaTeX Rendering (100% Local):**
  - **Lexical:** Tantivy BM25 with mathematical diacritic/transliteration folding (`Čech`, `Schrödinger`, `Poincaré`, `Erdős`, `Möbius`, `Łukasiewicz`).
  - **Semantic + Reranking:** Optional INT8 ONNX models (`multilingual-e5-small` / `bge-m3` embeddings via `usearch` HNSW on disk + `mmarco-mMiniLMv2` / `bge-reranker-v2-m3` cross-encoder) fused via Reciprocal Rank Fusion (RRF), with automatic idle unloading (`LazyModelSlot`) and pause/resume controls.
  - **Unified Filtering & Statements-Only Mode:** Tag, Topic Cluster, Collection, Author, Year range, Source Folder, Reading Status, Starred, Paper/Book category, and zero-ML **mathematical statement detection** (*Theorem, Lemma, Proposition, Corollary, Definition, Conjecture*) propagate directly into hybrid/BM25 search.
  - **Lazy KaTeX + Markdown:** `katex` (`.woff2`-only fonts) is dynamically imported only when math delimiters (`$...$`, `$$...$$`, `\(...\)`, `\[...\]`) are encountered — rendering formulas in Inspector abstracts, imported Nougat/Marker passages, viewer notes, and streaming LLM answers with zero cold-start penalty.
- **Bibliographic Metadata (14 Free Databases), Smart Paste & Global/Per-Collection Live `.bib` Sync:**
  - Configurable drag-and-drop priority waterfall across **14 free, zero-API-key bibliographic sources**: **AMS MRLookup** (toggleable single/bulk), **zbMATH Open** (`CC-BY-SA 4.0`), **Crossref**, **DBLP** (`CC0`), **OpenAlex** (`CC0`), **INSPIRE-HEP** (`CC0`), **arXiv**, **DataCite** (`CC0`), **Semantic Scholar**, **Europe PMC**, **Harvard LibraryCloud** (MARC21), **Library of Congress (SRU)**, **Google Books**, and **Open Library**, with per-source rate limiters, permanent local response cache, metadata verification checkmark (`✓`) lock, per-field locks, and guided title/author search.
  - **Smart Paste:** Single Inspector input automatically recognizing raw IDs or full URLs (DOI, arXiv, MR, zbMATH, ISBN-10/13) and pasted BibTeX entries.
  - **BibTeX / BibLaTeX & Live Auto-Sync:** Exact MRLookup alignment (`@article {MR...,`) or BibLaTeX dialect, customizable citation key patterns (`MR` default, `AuthorYear`, `AuthorYearWord`, `author_lower_year`), automatic collision disambiguation (`a`, `b`, `c`) on multi-exports, quick `\cite{key}` copy, and debounced atomic auto-sync to both a global `.bib` file and optional dedicated **per-collection `.bib` files** (right-click any document to add/remove from collections).
- **Related Documents, Suggested Classics & Preprint Alerts:**
  - On-the-fly **Similar in your library** (top-5 via mmapped USearch HNSW, zero extra indexing cost).
  - Local **Cites / Cited by** cross-linking and an on-demand **Suggested Classics** modal (filterable by *All / Papers / Books*) surfacing the most-cited works missing from your library from already-cached Crossref/Semantic Scholar references.
  - Discrete **Published version detected** badge when an `arXiv` preprint in the library has a published journal DOI/MR.
- **Catalog Organization, Maintenance & Updates:**
  - Global **Command Palette (`⌘K`)**, virtualized 60 fps catalog with multi-select (`Shift+Click` / `⌘+Click` / `Ctrl+Click`) and contextual batch actions (tag add/remove, verify/unverify, add to collection, export `.bib`/`.csv`, enqueue OCR).
  - Reorderable **Collections**, **Quick Views** (*Recent*, *Reading*, *Starred ★*), and **Saved Searches**.
  - **4-Level Duplicate Detection:** Exact BLAKE3, PDF structural/content hash, normalized metadata distance, and MinHash LSH fuzzy text similarity.
  - **Mathematical Classification:** Navigable **MSC2020** hierarchy (63 top-level classes), embedding-based k-NN suggestions, unsupervised spherical k-means topic clustering with editable labels, and free tags.
  - **Storage Health, Backups (`PAPERO_BKP_V2`) & Discrete Updates:** Streaming Zstd-compressed portable backups (preserving annotations, reading states, collections, saved views, BM25/vector indices, and sanitized preferences — never OS keychain keys or machine-specific `.bib` sync paths), built-in AppData size inspector + SQLite `VACUUM` + render cache clear in Settings, optional fast **front-pages OCR** (Settings → OCR), and discrete GitHub Releases update check.

### Release Sizing & Code-Signing Notes

- Targeted compressed installer size: **≤ 40 MB** for `.dmg`, `.deb`, `.exe`, and `.msi` (enforced by CI in `.github/workflows/release.yml`; the Linux `.AppImage` embeds WebKitGTK/GTK runtime libraries cleanly outside the 40 MB native cap).
- **Windows Static CRT:** `papero-app.exe` statically links the MSVC C/C++ runtime (`/MT` via `.cargo/config.toml`), verified in CI, while staging the required CRT DLLs exclusively alongside `onnxruntime.dll`.
- **Note on Free Open-Source Code Signing (SignPath Foundation):**
  **SignPath Foundation** provides free code signing certificates for qualifying open-source projects on GitHub, acting as the legal publisher for signed releases. SignPath is **not** integrated into this repository's CI until approved and explicitly configured by the project maintainer.
- **Linux *"Show in Folder"*:** Papero attempts `org.freedesktop.FileManager1.ShowItems` via D-Bus with a 2-second timeout and falls back to `xdg-open`; you can force direct `xdg-open` in **Settings → Visualizzatore & Lettori**.

---

## Optional External Tools

### 1. Tesseract OCR (for Scanned PDF & DjVu Files)

Papero bundles DjVuLibre (`ddjvu`, `djvused`, `djvutxt`) and PDFium directly, but **does not bundle Tesseract** (~50–100+ MB with traineddata). If you have scanned mathematics books or papers without a text layer:

- **macOS (Homebrew):**
  ```bash
  brew install tesseract tesseract-lang
  ```
- **Windows:**
  Install [Tesseract at UB Mannheim](https://github.com/UB-Mannheim/tesseract/wiki), select desired math/language packs (`eng`, `ita`, `fra`, `deu`, `rus`) during installation, and either add Tesseract to `PATH` or point **Settings → OCR (Tesseract) → Percorso tesseract (opzionale)** to `C:\Program Files\Tesseract-OCR\tesseract.exe`.
- **Ubuntu / Debian:**
  ```bash
  sudo apt install tesseract-ocr tesseract-ocr-eng tesseract-ocr-ita tesseract-ocr-fra tesseract-ocr-deu tesseract-ocr-rus
  ```

Once installed, Papero automatically detects Tesseract (`--version` and `--list-langs`) and lets you run OCR on single documents, multi-selected batches, or all pending scanned files without touching source files.

### 2. Nougat / Marker (Optional Formula LaTeX Import)

Tesseract recognizes prose well on clean scans but often struggles with multi-line mathematical equations, commutative diagrams, and fractions. You can optionally run **Meta Nougat** or **Marker** locally on dense papers to generate `.mmd` / `.md` LaTeX output, then click **Importa LaTeX (.mmd/.md)** in Papero's right-hand Inspector.

- Example with [Nougat](https://github.com/facebookresearch/nougat):
  ```bash
  pip install nougat-ocr
  nougat /path/to/paper.pdf -o ./nougat_out
  ```
- Example with [Marker](https://github.com/VikParuchuri/marker):
  ```bash
  pip install marker-pdf
  marker_single /path/to/paper.pdf --output_dir ./marker_out
  ```
- Papero stores imported LaTeX snippets as dedicated formula passages (`is_alt_formula = 1`), rendered via lazy KaTeX and preserved even if you re-run Tesseract OCR later.

---

## Bibliographic Sources & Optional LLM Integration

### Bibliographic Databases (14 Free Sources)
- **Crossref, OpenAlex & Open Library polite pool:** In the first-run wizard or Settings, you may optionally enter an email address saved strictly locally (in your OS credential store when available). Papero includes **no hardcoded developer email**.
- **AMS MRLookup:** Free and unauthenticated, toggleable separately for single lookups and rate-limited bulk scans (`1 req/5s`) in respect of AMS usage guidelines.
- **zbMATH Open:** Metadata is redistributed under `CC-BY-SA 4.0` (FIZ Karlsruhe).
- **DBLP, OpenAlex, INSPIRE-HEP & DataCite:** Open bibliographic metadata (`CC0 1.0`), alongside **arXiv**, **Semantic Scholar**, **Europe PMC**, **Harvard LibraryCloud**, **Library of Congress (SRU)**, **Google Books**, and **Open Library** — all queried free of charge with zero API keys required.

### Generative LLM Providers: Streaming, Document Q&A & Privacy

All core Papero features (search, semantic embeddings, reranking, classification, viewer, annotations, BibTeX, duplicates, OCR) are **100% local and free without any LLM**.

You can optionally enable an **on-demand** LLM backend in **Settings → LLM (Opzionale)** (`Disattivato` by default) to stream grounded Markdown + KaTeX answers token-by-token both across search results (`[Doc #id, p. X]`) and directly inside the viewer (**"Ask this document / page / selection"** with clickable `[p. X]` citations):

- **Consumer web subscriptions DO NOT work for desktop API apps:** Plans such as **ChatGPT Plus / Pro**, **Claude Pro / Max**, and **Google One AI Premium (Gemini Advanced)** only cover web/mobile chat apps and **do not include developer API credits**.
- **Cloud providers require your own API key (saved strictly in OS Keyring):**
  - **Anthropic (Claude API):** <https://console.anthropic.com/settings/keys>
  - **OpenAI API:** <https://platform.openai.com/api-keys>
  - **Google Gemini API (AI Studio):** <https://aistudio.google.com/apikey>
  API requests go directly from your machine to the selected provider (`api.anthropic.com`, `api.openai.com`, `generativelanguage.googleapis.com`) using live model discovery (`/v1/models` or `/v1beta/models`), with **no hardcoded model lists, no third-party proxy, and keys never written to config files, logs, or backups**.
- **Privacy & Google Gemini Free Tier notice:** Commercial Anthropic and OpenAI API inputs are not used for model training by default. Google AI Studio's rate-limited **Free Tier (Unpaid)** terms allow Google to review and use submitted prompts/outputs for product improvement (whereas Paid Tier projects do not train on API inputs). Papero enforces per-folder and per-document cloud exclusions (`exclude_from_cloud_llm`), shows a pre-send token estimator, caps passages/tokens, and requires explicit confirmation before the first cloud transmission.
- **Zero-cost private alternative (Ollama):** Install [Ollama](https://ollama.com/) locally (`http://127.0.0.1:11434`, live models fetched via `GET /api/tags`) to run open-weight models completely offline with streaming, zero API costs, and zero network egress.

---

## Building from Source

### Prerequisites

- **Rust** (stable toolchain via `rustup`)
- **Node.js 22+** and `npm`
- **Tauri v2 system prerequisites**:
  - **Linux (Ubuntu/Debian)**:
    ```bash
    sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
      libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf djvulibre-bin
    ```
  - **macOS**: Xcode Command Line Tools + `brew install djvulibre` (for staging DjVuLibre binaries)
  - **Windows**: Visual Studio C++ Build Tools + WebView2 Runtime (built into Windows 10/11)

### 1. Stage Bundled Native Resources

Download pinned non-V8 PDFium (`bblanchon/pdfium-binaries`), CPU ONNX Runtime (CoreML-enabled on macOS, no `DirectML.dll` on Windows), DjVuLibre standalone tools + relocatable libraries, and GPLv2 compliance notices into `src-tauri/resources/`:

- **macOS / Linux:**
  ```bash
  ./scripts/stage-resources.sh --target aarch64-apple-darwin   # or x86_64-apple-darwin / x86_64-unknown-linux-gnu
  ```
- **Windows (PowerShell):**
  ```powershell
  .\scripts\stage-resources.ps1
  ```

### 2. Test & Build

```bash
# Frontend check, tests, and bundle
npm --prefix frontend ci
npm --prefix frontend run check
npm --prefix frontend run test
npm --prefix frontend run build

# Rust lint & tests
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Build native installer from repository root
npm --prefix frontend install --no-save @tauri-apps/cli@^2
./frontend/node_modules/.bin/tauri build          # macOS / Ubuntu
# .\frontend/node_modules/.bin/tauri.cmd build    # Windows PowerShell
```

---

## License & Compliance

- Papero application code: dual-licensed **MIT OR Apache-2.0** ([`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE)).
- Bundled third-party components and licenses are detailed in [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).
- Standalone **DjVuLibre 3.5.30** utilities (`ddjvu`, `djvused`, `djvutxt`) are bundled strictly as separate OS executables under **GPL-2.0-or-later** ([`licenses/GPL-2.0-DjVuLibre.txt`](licenses/GPL-2.0-DjVuLibre.txt), written source offer in [`licenses/SOURCE_OFFER.txt`](licenses/SOURCE_OFFER.txt)).
