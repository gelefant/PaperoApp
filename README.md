# Papero

**Papero** is a 100% local, zero-cost, cross-platform desktop cataloger for mathematical books and research papers (**PDF** and **DjVu**), combining exact bibliographic metadata, fast full-text + neural semantic search, MSC2020 classification, and an instant 800-page internal viewer.

> **Read-Only Guarantee on Your Library:** Papero treats all configured source folders as **strictly read-only**. It never edits, moves, renames, injects annotations into, or deletes your PDF/DjVu files. All derived caches, indices, thumbnails, OCR text, and SQLite databases live inside Papero's application data directory.

---

## Key Features

- **Incremental BLAKE3 Rescan & Source Archive**: Tracks 7 file states (unchanged, new, copy, moved, modified, removed with manual-metadata restoration, offline/unmounted drive preserved without purging metadata).
- **Instant Unified PDF & DjVu Viewer**:
  - Byte-range streaming (`papero-doc://` HTTP `206 Partial Content`) opens 800+ page PDFs via `pdf.js` in under a second without loading whole files into RAM.
  - Background `ddjvu` rasterization with bounded disk LRU cache + invisible selectable text layer from `djvused` S-expressions.
  - Clickable passage highlights (`[0,1]` top-left normalized bounding boxes), bookmarks/outline, printed vs ordinal page labels, zoom controls, optional detached viewer window, and OS external reader presets (`DjView`, `Skim`, `SumatraPDF`, `Okular`, `Evince`, `Zathura`, macOS `Preview`).
- **Hybrid Search (100% Local)**:
  - **Lexical**: Tantivy BM25 with custom mathematical diacritic/transliteration folding (`Čech`, `Schrödinger`, `Poincaré`, `Erdős`, `Möbius`).
  - **Semantic + Reranking**: Optional INT8 ONNX models (`multilingual-e5-small` / `bge-m3` embeddings via `usearch` HNSW on disk + `mmarco-mMiniLMv2` / `bge-reranker-v2-m3` cross-encoder) fused via Reciprocal Rank Fusion (RRF), with automatic idle unloading from RAM (`LazyModelSlot`).
- **Bibliographic Metadata & Exact MRLookup BibTeX**:
  - Waterfall client supporting **11 free, zero-API-key databases**: **AMS MRLookup** (toggleable), **zbMATH Open** (`CC-BY-SA 4.0`), **DBLP** (`CC0`), **OpenAlex** (`CC0`), **Crossref**, **arXiv**, **Semantic Scholar**, **INSPIRE-HEP** (`CC0`), **DataCite** (`CC0`), **Europe PMC**, and **Open Library** (books only), with strict per-source rate limiters, one-click bulk metadata scan, and permanent local cache.
  - Field locks protect manually edited metadata from autofetch overwrites, and BibTeX exports match MRLookup's exact column alignment (`@article {MR...,` syntax + LaTeX escaping).
- **4-Level Duplicate Detection**: Exact BLAKE3, PDF structural/content hash, normalized metadata distance, and MinHash LSH fuzzy text similarity.
- **Mathematical Classification**: Navigable **MSC2020** hierarchy (63 top-level classes), embedding-based k-NN suggestions, unsupervised spherical k-means topic clustering with editable labels, and free tags.
- **OCR & Formula Support**: Optional external **Tesseract OCR** integration (TSV line chunks with coordinates, preserving original files untouched) + **Nougat / Marker** Markdown/LaTeX formula import.
- **Complete Desktop UX**: 4-step first-run wizard, virtualized three-pane layout, Light/Dark themes, full Italian & English localization, BibTeX/CSL-JSON/CSV import-export, and verified one-click backup/restore.

---

## Installation (Pre-built Releases)

Download the installer for your OS from the repository's **GitHub Releases** page (targeted compressed installer size: **≤ 40 MB**).

### macOS (Apple Silicon `aarch64` & Intel `x64`)

Choose the matching `.dmg` (`aarch64-apple-darwin` for M1/M2/M3/M4 Macs, `x86_64-apple-darwin` for Intel Macs), open it, and drag **Papero.app** to `/Applications`.

Because Papero strictly adheres to a **zero-cost open-source model** without a paid Apple Developer ID subscription, releases use free ad-hoc signing (`codesign --force --deep --sign -`) and are **not notarized by Apple**.

On **macOS Sequoia (15) and later** (where Control-Click -> Open no longer overrides Gatekeeper directly):

1. Double-click **Papero.app** once (macOS will display a warning that Apple could not verify the app is free of malware). Dismiss the dialog.
2. Open **System Settings -> Privacy & Security**, scroll down to the Security section, and click **Open Anyway** next to `"Papero.app" was blocked...`, then confirm with your administrator password.
3. *Terminal alternative (before first launch):*
   ```bash
   xattr -dr com.apple.quarantine /Applications/Papero.app
   ```

### Windows 10 / 11 (`x64` `.exe` / `.msi`)

Run the downloaded NSIS `.exe` or `.msi` installer.

Because the binary is unsigned (no paid Authenticode EV certificate), Microsoft Defender SmartScreen will show `"Windows protected your PC"`:

1. Click **More info** (*Ulteriori informazioni*).
2. Click **Run anyway** (*Esegui comunque*).

> **Note on Free Open-Source Code Signing (SignPath Foundation):**
> **SignPath Foundation** provides free code signing certificates for qualifying open-source projects on GitHub, acting as the legal publisher for signed releases. Typical eligibility requirements include: a public open-source repository under an OSI-approved license (`MIT`/`Apache-2.0`), an established public commit and release history, compliance with SignPath's Code Signing Policy, and manual/CI approval flows owned by maintainers. SignPath is **not** integrated into this repository's CI until approved and explicitly configured by the project maintainer.

### Linux (`x64` `.deb` & `.AppImage`, Ubuntu 22.04+)

- **Debian / Ubuntu (`.deb`, ≤ 40 MB using system WebKitGTK):**
  ```bash
  sudo apt install ./Papero_*.deb
  ```
- **Portable `.AppImage`** *(bundles WebKitGTK runtime libs, so file size exceeds 40 MB)*:
  ```bash
  chmod +x Papero_*.AppImage
  ./Papero_*.AppImage
  ```
- Note on *"Show in Folder"*: Papero attempts `org.freedesktop.FileManager1.ShowItems` via D-Bus with a 2-second timeout and falls back to `xdg-open`; you can force direct `xdg-open` in **Settings -> Visualizzatore & Lettori**.

---

## Optional External Tools

### 1. Tesseract OCR (for Scanned PDF & DjVu Files)

Papero bundles DjVuLibre (`ddjvu`, `djvused`, `djvutxt`) and PDFium directly, but **does not bundle Tesseract** (~50–100+ MB with traineddata). If you have scanned mathematics books or papers without a text layer:

- **macOS (Homebrew):**
  ```bash
  brew install tesseract tesseract-lang
  ```
- **Windows:**
  Install [Tesseract at UB Mannheim](https://github.com/UB-Mannheim/tesseract/wiki), select desired math/language packs (`eng`, `ita`, `fra`, `deu`, `rus`) during installation, and either add Tesseract to `PATH` or point **Settings -> OCR (Tesseract) -> Percorso binario** to `C:\Program Files\Tesseract-OCR\tesseract.exe`.
- **Ubuntu / Debian:**
  ```bash
  sudo apt install tesseract-ocr tesseract-ocr-eng tesseract-ocr-ita tesseract-ocr-fra tesseract-ocr-deu tesseract-ocr-rus
  ```

Once installed, Papero automatically detects Tesseract (`--version` and `--list-langs`) and lets you run OCR on single documents (right-click -> *Esegui OCR (Tesseract)*) or batch-process all scanned files in the background without touching source files.

### 2. Nougat / Marker (Optional Formula LaTeX Import)

Tesseract recognizes prose well on clean scans but often struggles with multi-line mathematical equations, commutative diagrams, and fractions. You can optionally run **Meta Nougat** or **Marker** locally on dense papers to generate `.mmd` / `.md` LaTeX output, then click **Importa Markdown/LaTeX (Nougat/Marker)** in Papero's right-hand Inspector.

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
- Papero stores imported LaTeX snippets as dedicated formula passages (`is_alt_formula = 1`), preserving them even if you re-run Tesseract OCR later.

---

## Bibliographic Sources & Optional LLM Integration

### Bibliographic Databases
- **Crossref, OpenAlex & Open Library polite pool**: In the first-run wizard or Settings, you may optionally enter an email address saved strictly locally (in your OS credential store when available). Papero includes **no hardcoded developer email**.
- **AMS MRLookup**: Free and unauthenticated, enabled via toggle; please avoid heavy automated bulk queries against MRLookup in respect of AMS usage guidelines.
- **zbMATH Open**: Metadata is redistributed under `CC-BY-SA 4.0` (FIZ Karlsruhe).
- **DBLP, OpenAlex, INSPIRE-HEP & DataCite**: Open bibliographic metadata (`CC0 1.0`), alongside **arXiv**, **Semantic Scholar**, and **Europe PMC** — all queried free of charge with zero API keys required.

### Generative LLM Providers: API Keys vs Consumer Subscriptions

All core Papero features (search, semantic embeddings, reranking, classification, viewer, BibTeX, duplicates, OCR) are **100% local and free without any LLM**.

You can optionally enable an **on-demand** LLM backend in **Settings -> LLM (Opzionale)** (`Disattivato` by default) to synthesize grounded answers with clickable `[Doc #id, p. X]` inline citations directly over your retrieved local passages:

- **Consumer web subscriptions DO NOT work for desktop API apps:** Plans such as **ChatGPT Plus / Pro**, **Claude Pro / Max**, and **Google One AI Premium (Gemini Advanced)** only cover web/mobile chat apps and **do not include developer API credits**.
- **Cloud providers require your own API key (saved strictly in OS Keyring):**
  - **Anthropic (Claude API):** <https://console.anthropic.com/settings/keys>
  - **OpenAI API:** <https://platform.openai.com/api-keys>
  - **Google Gemini API (AI Studio):** <https://aistudio.google.com/apikey>
  API requests go directly from your machine to the selected provider (`api.anthropic.com`, `api.openai.com`, `generativelanguage.googleapis.com`) using live model discovery (`/v1/models` or `/v1beta/models`), with **no hardcoded model lists, no third-party proxy, and keys never written to config files, logs, or backups**.
- **Privacy & Google Gemini Free Tier notice:** Commercial Anthropic and OpenAI API inputs are not used for model training by default. Google AI Studio's rate-limited **Free Tier (Unpaid)** terms allow Google to review and use submitted prompts/outputs for product improvement (whereas Paid Tier projects do not train on API inputs). Papero provides per-folder and per-document cloud exclusions (`exclude_from_cloud_llm`), shows a pre-send token estimator, enforces configurable passage/token caps, and requires explicit confirmation before the first cloud transmission.
- **Zero-cost private alternative (Ollama):** Install [Ollama](https://ollama.com/) locally (`http://127.0.0.1:11434`, live models fetched via `GET /api/tags`) to run open-weight models completely offline with zero API costs and zero network egress.

---

## Building from Source

### Prerequisites

- **Rust** (stable toolchain via `rustup`)
- **Node.js 20+** and `npm`
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
# .\frontend\node_modules\.bin\tauri.cmd build    # Windows PowerShell
```

---

## License & Compliance

- Papero application code: dual-licensed **MIT OR Apache-2.0** ([`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE)).
- Bundled third-party components and licenses are detailed in [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).
- Standalone **DjVuLibre 3.5.30** utilities (`ddjvu`, `djvused`, `djvutxt`) are bundled strictly as separate OS executables under **GPL-2.0-or-later** ([`licenses/GPL-2.0-DjVuLibre.txt`](licenses/GPL-2.0-DjVuLibre.txt), written source offer in [`licenses/SOURCE_OFFER.txt`](licenses/SOURCE_OFFER.txt)).
