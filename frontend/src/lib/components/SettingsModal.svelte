<script lang="ts">
  import { untrack } from 'svelte';
  import { save as saveFileDialog } from '@tauri-apps/plugin-dialog';
  import { api } from '../api';
  import { APP_VERSION } from '../appVersion';
  import { sanitizeUiSettings } from '../settingsUtils';
  import type {
    BibtexFormat,
    BibtexKeyPattern,
    DownloadProgressEvent,
    ExternalReaderPreset,
    LlmKeyStatus,
    LlmModelInfo,
    LlmProvider,
    MlEnvironmentStatus,
    OcrEnvironmentStatus,
    SettingsTabId,
    SourceRoot,
    UiSettingsPayload,
  } from '../types';
  import { normalizeLang, tr } from '../i18n';

  interface Props {
    open: boolean;
    initialTab?: SettingsTabId | null;
    settings: UiSettingsPayload;
    roots: SourceRoot[];
    presets: ExternalReaderPreset[];
    mlStatus: MlEnvironmentStatus | null;
    mlDownloadProgress: DownloadProgressEvent | null;
    ocrStatus?: OcrEnvironmentStatus | null;
    onSaveSettings: (next: UiSettingsPayload) => Promise<void>;
    onToggleRoot: (rootId: string, enabled: boolean) => Promise<void>;
    onSetRootCloudExclusion?: (rootId: string, excluded: boolean) => Promise<void>;
    onRebindRoot?: (rootId: string) => Promise<void>;
    onDeleteRoot: (rootId: string) => Promise<void>;
    onAddFolder: () => Promise<void>;
    onDownloadMlProfile: (profile: 'light' | 'quality') => Promise<void>;
    onUnloadMlModels: () => Promise<void>;
    onRefreshMlStatus: () => void;
    onRefreshOcrStatus?: () => void;
    onRunPendingOcr?: () => Promise<void>;
    backupBusy?: boolean;
    backupNotice?: string;
    backupError?: string;
    onCreateBackup: () => Promise<void>;
    onUpdateBackupNow?: () => Promise<void>;
    onRestoreBackup: () => Promise<void>;
    onLaunchWizard: () => void;
    onClose: () => void;
  }

  let {
    open,
    initialTab = null,
    settings,
    roots,
    presets,
    mlStatus,
    mlDownloadProgress,
    ocrStatus = null,
    onSaveSettings,
    onToggleRoot,
    onSetRootCloudExclusion,
    onRebindRoot,
    onDeleteRoot,
    onAddFolder,
    onDownloadMlProfile,
    onUnloadMlModels,
    onRefreshMlStatus,
    onRefreshOcrStatus,
    onRunPendingOcr,
    backupBusy = false,
    backupNotice = '',
    backupError = '',
    onCreateBackup,
    onUpdateBackupNow,
    onRestoreBackup,
    onLaunchWizard,
    onClose,
  }: Props = $props();

  type TabId = SettingsTabId;

  const DEFAULT_SOURCE_PRIORITY = [
    'mrlookup',
    'zbmath',
    'crossref',
    'dblp',
    'openalex',
    'inspirehep',
    'arxiv',
    'datacite',
    'semanticscholar',
    'europepmc',
    'harvard',
    'loc',
    'googlebooks',
    'openlibrary',
  ] as const;

  type BoolSettingKey =
    | 'zbmath_enabled'
    | 'dblp_enabled'
    | 'crossref_enabled'
    | 'openalex_enabled'
    | 'datacite_enabled'
    | 'inspirehep_enabled'
    | 'arxiv_enabled'
    | 'semanticscholar_enabled'
    | 'europepmc_enabled'
    | 'harvard_enabled'
    | 'loc_enabled'
    | 'googlebooks_enabled'
    | 'openlibrary_enabled';

  const SOURCE_META: Record<
    string,
    { label: string; descIt: string; descEn: string; toggleKey?: BoolSettingKey }
  > = {
    mrlookup: {
      label: 'AMS MRLookup (MathSciNet)',
      descIt: 'BibTeX AMS originale, MRNumber, MSC (limite AMS: 1 rich./5s)',
      descEn: 'Verbatim AMS BibTeX, MRNumber, MSC (AMS limit: 1 req/5s)',
    },
    zbmath: {
      label: 'zbMATH Open',
      descIt: 'Matematica / Analisi Numerica, Zbl, MSC2020',
      descEn: 'Mathematics / Numerical Analysis, Zbl, MSC2020',
      toggleKey: 'zbmath_enabled',
    },
    crossref: {
      label: 'Crossref',
      descIt: 'DOI e metadati ufficiali degli editori',
      descEn: 'DOIs and publisher journal metadata',
      toggleKey: 'crossref_enabled',
    },
    dblp: {
      label: 'DBLP',
      descIt: 'Informatica / Computer Science',
      descEn: 'Computer Science bibliography',
      toggleKey: 'dblp_enabled',
    },
    openalex: {
      label: 'OpenAlex',
      descIt: 'Catalogo scientifico multidisciplinare',
      descEn: 'Multidisciplinary scholarly catalog',
      toggleKey: 'openalex_enabled',
    },
    inspirehep: {
      label: 'INSPIRE-HEP',
      descIt: 'Fisica Matematica / Alte Energie',
      descEn: 'Mathematical Physics / High-Energy Physics',
      toggleKey: 'inspirehep_enabled',
    },
    arxiv: {
      label: 'arXiv Atom',
      descIt: 'Preprint arXiv',
      descEn: 'arXiv preprints',
      toggleKey: 'arxiv_enabled',
    },
    datacite: {
      label: 'DataCite',
      descIt: 'DOI arXiv / ETH / Zenodo',
      descEn: 'arXiv / ETH / Zenodo DOIs',
      toggleKey: 'datacite_enabled',
    },
    semanticscholar: {
      label: 'Semantic Scholar',
      descIt: 'Informatica / Machine Learning',
      descEn: 'Computer Science / Machine Learning',
      toggleKey: 'semanticscholar_enabled',
    },
    europepmc: {
      label: 'Europe PMC',
      descIt: 'Biomatematica / Scienze della vita',
      descEn: 'Biomathematics / Life Sciences',
      toggleKey: 'europepmc_enabled',
    },
    harvard: {
      label: 'Harvard LibraryCloud',
      descIt: 'Catalogo universitario MARC21 (editore, città, collana, edizione, pagine, ISBN)',
      descEn: 'University MARC21 catalog (publisher, place, series, edition, pages, ISBN)',
      toggleKey: 'harvard_enabled',
    },
    loc: {
      label: 'Library of Congress (SRU)',
      descIt: 'Catalogo ufficiale LoC per libri e monografie accademiche',
      descEn: 'Official LoC catalog for academic books & monographs',
      toggleKey: 'loc_enabled',
    },
    googlebooks: {
      label: 'Google Books',
      descIt: 'Libri scientifici e commerciali (editore, pagine, ISBN)',
      descEn: 'Academic & commercial books (publisher, pages, ISBN)',
      toggleKey: 'googlebooks_enabled',
    },
    openlibrary: {
      label: 'Open Library',
      descIt: 'Fallback finale per libri e monografie (ISBN)',
      descEn: 'Final fallback for books & monographs (ISBN)',
      toggleKey: 'openlibrary_enabled',
    },
  };

  function normalizeSourcePriority(raw?: string[] | null): string[] {
    const out: string[] = [];
    for (const item of raw ?? []) {
      const k = item.trim().toLowerCase();
      if ((DEFAULT_SOURCE_PRIORITY as readonly string[]).includes(k) && !out.includes(k)) {
        out.push(k);
      }
    }
    for (const def of DEFAULT_SOURCE_PRIORITY) {
      if (!out.includes(def)) {
        if (def === 'harvard' || def === 'loc' || def === 'googlebooks') {
          const olIdx = out.indexOf('openlibrary');
          if (olIdx !== -1) {
            out.splice(olIdx, 0, def);
            continue;
          }
        }
        out.push(def);
      }
    }
    return out;
  }

  let activeTab = $state<TabId>('general');
  let draft = $state<UiSettingsPayload>(
    untrack(() => ({
      ...settings,
      metadata_source_priority: normalizeSourcePriority(settings.metadata_source_priority),
    }))
  );
  let dragSourceIdx = $state<number | null>(null);
  let dragOverIdx = $state<number | null>(null);
  let emailInput = $state(untrack(() => settings.polite_email ?? ''));
  let saving = $state(false);
  let downloading = $state<string | null>(null);
  let mlError = $state('');
  let statusNotice = $state('');
  let llmKeyStatus = $state<LlmKeyStatus>({
    anthropic_present: false,
    openai_present: false,
    gemini_present: false,
  });
  let llmKeyInput = $state('');
  let llmModels = $state<LlmModelInfo[]>([]);
  let llmBusy = $state(false);
  let llmMsg = $state('');
  let llmErr = $state('');
  let bibSyncBusy = $state(false);
  let bibSyncMsg = $state('');

  function citeKeyPreview(pattern?: BibtexKeyPattern): string {
    switch (pattern) {
      case 'author_year':
        return 'Poincare1895';
      case 'author_year_word':
        return 'Poincare1895Analysis';
      case 'author_lower_year':
        return 'poincare1895';
      default:
        return 'MR1234567';
    }
  }

  async function handleChooseBibSyncPath() {
    try {
      const chosen = await saveFileDialog({
        defaultPath: draft.bibtex_auto_sync_path || 'papero-library.bib',
        filters: [{ name: 'BibTeX', extensions: ['bib'] }],
      });
      if (chosen) {
        draft.bibtex_auto_sync_path = chosen;
        draft.bibtex_auto_sync_enabled = true;
        bibSyncMsg = '';
      }
    } catch (e) {
      bibSyncMsg = String(e);
    }
  }

  async function handleSyncBibNow() {
    bibSyncBusy = true;
    bibSyncMsg = '';
    try {
      await saveDraft();
      const rep = await api.syncBibtexNow();
      bibSyncMsg =
        lang === 'en'
          ? `Synced ${rep.count} entries to ${rep.path}`
          : `Sincronizzate ${rep.count} voci in ${rep.path}`;
    } catch (e) {
      bibSyncMsg = String(e);
    } finally {
      bibSyncBusy = false;
    }
  }

  $effect(() => {
    if (open) {
      if (initialTab) {
        activeTab = initialTab;
      }
      draft = {
        ...settings,
        metadata_source_priority: normalizeSourcePriority(settings.metadata_source_priority),
      };
      emailInput = settings.polite_email ?? '';
      mlError = '';
      statusNotice = '';
      llmKeyInput = '';
      llmModels = [];
      llmMsg = '';
      llmErr = '';
      onRefreshMlStatus();
      onRefreshOcrStatus?.();
      api
        .getLlmKeyStatus()
        .then((st) => {
          llmKeyStatus = st;
        })
        .catch(() => {});
    }
  });

  function isCloudProvider(p: LlmProvider): boolean {
    return p === 'anthropic' || p === 'openai' || p === 'gemini';
  }

  function hasKeyForProvider(p: LlmProvider): boolean {
    if (p === 'anthropic') return llmKeyStatus.anthropic_present;
    if (p === 'openai') return llmKeyStatus.openai_present;
    if (p === 'gemini') return llmKeyStatus.gemini_present;
    return false;
  }

  function hasConsentForProvider(p: LlmProvider): boolean {
    if (p === 'anthropic') return draft.llm_consent_anthropic;
    if (p === 'openai') return draft.llm_consent_openai;
    if (p === 'gemini') return draft.llm_consent_gemini;
    return true;
  }

  function revokeConsentForProvider(p: LlmProvider) {
    if (p === 'anthropic') draft.llm_consent_anthropic = false;
    else if (p === 'openai') draft.llm_consent_openai = false;
    else if (p === 'gemini') draft.llm_consent_gemini = false;
  }

  function handleProviderChange(next: LlmProvider) {
    draft.llm_provider = next;
    draft.llm_model = next === settings.llm_provider ? settings.llm_model : null;
    llmKeyInput = '';
    llmModels = [];
    llmMsg = '';
    llmErr = '';
  }

  async function refreshLlmKeyStatus() {
    llmKeyStatus = await api.getLlmKeyStatus();
  }

  async function handleSaveLlmKey() {
    if (!isCloudProvider(draft.llm_provider)) return;
    llmBusy = true;
    llmMsg = '';
    llmErr = '';
    try {
      await api.saveLlmApiKey(draft.llm_provider, llmKeyInput);
      llmKeyInput = '';
      await refreshLlmKeyStatus();
      llmMsg =
        lang === 'en'
          ? 'API key saved securely in OS Keychain.'
          : 'Chiave API salvata in modo sicuro nel Portachiavi di sistema.';
    } catch (e) {
      llmErr = String(e);
    } finally {
      llmBusy = false;
    }
  }

  async function handleRemoveLlmKey() {
    if (!isCloudProvider(draft.llm_provider)) return;
    llmBusy = true;
    llmMsg = '';
    llmErr = '';
    try {
      await api.removeLlmApiKey(draft.llm_provider);
      llmKeyInput = '';
      llmModels = [];
      await refreshLlmKeyStatus();
      llmMsg =
        lang === 'en'
          ? 'API key removed from OS Keychain.'
          : 'Chiave API rimossa dal Portachiavi di sistema.';
    } catch (e) {
      llmErr = String(e);
    } finally {
      llmBusy = false;
    }
  }

  async function handleVerifyLlmKey() {
    if (draft.llm_provider === 'disabled') return;
    llmBusy = true;
    llmMsg = '';
    llmErr = '';
    try {
      const res = await api.verifyLlmKey(
        draft.llm_provider,
        llmKeyInput.trim() || null,
        draft.llm_ollama_url
      );
      if (res.ok) {
        llmModels = res.models;
        if (
          res.models.length > 0 &&
          (!draft.llm_model || !res.models.some((m) => m.id === draft.llm_model))
        ) {
          draft.llm_model = res.models[0].id;
        }
        llmMsg = res.message;
      } else {
        llmErr = res.message;
      }
    } catch (e) {
      llmErr = String(e);
    } finally {
      llmBusy = false;
    }
  }

  async function handleFetchLlmModels() {
    if (draft.llm_provider === 'disabled') return;
    llmBusy = true;
    llmMsg = '';
    llmErr = '';
    try {
      const models = await api.listLlmModels(
        draft.llm_provider,
        draft.llm_ollama_url,
        llmKeyInput.trim() || null
      );
      llmModels = models;
      if (models.length === 0) {
        llmMsg =
          lang === 'en'
            ? 'Endpoint reachable, but no models returned.'
            : 'Endpoint raggiungibile, nessun modello restituito.';
      } else {
        if (!draft.llm_model || !models.some((m) => m.id === draft.llm_model)) {
          draft.llm_model = models[0].id;
        }
        llmMsg =
          lang === 'en'
            ? `Loaded ${models.length} live models from provider.`
            : `Caricati ${models.length} modelli reali dal provider.`;
      }
    } catch (e) {
      llmErr = String(e);
    } finally {
      llmBusy = false;
    }
  }

  let lang = $derived(normalizeLang(draft.ui_language));

  function moveSourcePriority(idx: number, delta: number) {
    const list = normalizeSourcePriority(draft.metadata_source_priority);
    const target = idx + delta;
    if (idx < 0 || idx >= list.length || target < 0 || target >= list.length) return;
    const [moved] = list.splice(idx, 1);
    list.splice(target, 0, moved);
    draft.metadata_source_priority = list;
  }

  function moveSourceToTop(idx: number) {
    const list = normalizeSourcePriority(draft.metadata_source_priority);
    if (idx <= 0 || idx >= list.length) return;
    const [moved] = list.splice(idx, 1);
    list.unshift(moved);
    draft.metadata_source_priority = list;
  }

  function handleSourceDrop(targetIdx: number) {
    if (dragSourceIdx === null || dragSourceIdx === targetIdx) {
      dragSourceIdx = null;
      dragOverIdx = null;
      return;
    }
    const list = normalizeSourcePriority(draft.metadata_source_priority);
    const [moved] = list.splice(dragSourceIdx, 1);
    list.splice(targetIdx, 0, moved);
    draft.metadata_source_priority = list;
    dragSourceIdx = null;
    dragOverIdx = null;
  }

  function resetSourcePriority() {
    draft.metadata_source_priority = [...DEFAULT_SOURCE_PRIORITY];
  }

  function isSourceActive(srcId: string): boolean {
    if (!draft.network_lookups_enabled) return false;
    if (srcId === 'mrlookup') {
      return draft.mrlookup_enabled_single || draft.mrlookup_enabled_bulk;
    }
    const key = SOURCE_META[srcId]?.toggleKey;
    return key ? Boolean(draft[key]) : false;
  }

  async function saveDraft() {
    saving = true;
    statusNotice = '';
    try {
      const sanitized = sanitizeUiSettings(
        {
          ...draft,
          polite_email: emailInput.trim() || null,
        },
        settings
      );
      draft = sanitized;
      await onSaveSettings(sanitized);
      statusNotice = lang === 'en' ? 'Settings saved.' : 'Impostazioni salvate.';
    } catch (e) {
      statusNotice = String(e);
    } finally {
      saving = false;
    }
  }

  async function handleDownload(profile: 'light' | 'quality') {
    downloading = profile;
    mlError = '';
    try {
      await onDownloadMlProfile(profile);
    } catch (e) {
      mlError = String(e);
    } finally {
      downloading = null;
    }
  }
</script>

{#if open}
  <div class="modal-backdrop" role="dialog" aria-modal="true" aria-label={tr(lang, 'settingsTitle')}>
    <div class="settings-modal">
      <header class="modal-head">
        <h2>{tr(lang, 'settingsTitle')} <span class="mono" style="font-weight: 400; font-size: 0.78rem; color: var(--text-muted); margin-left: 0.35rem;">v{APP_VERSION}</span></h2>
        <button type="button" onclick={onClose}>{tr(lang, 'close')}</button>
      </header>

      <div class="modal-body">
        <nav class="tab-nav">
          <button type="button" class:active={activeTab === 'general'} onclick={() => (activeTab = 'general')}>
            {tr(lang, 'tabGeneral')}
          </button>
          <button type="button" class:active={activeTab === 'folders'} onclick={() => (activeTab = 'folders')}>
            {tr(lang, 'tabFolders')}
          </button>
          <button type="button" class:active={activeTab === 'ml'} onclick={() => (activeTab = 'ml')}>
            {tr(lang, 'tabMl')}
          </button>
          <button type="button" class:active={activeTab === 'ocr'} onclick={() => (activeTab = 'ocr')}>
            OCR (Tesseract)
          </button>
          <button type="button" class:active={activeTab === 'metadata'} onclick={() => (activeTab = 'metadata')}>
            {tr(lang, 'tabMetadata')}
          </button>
          <button type="button" class:active={activeTab === 'llm'} onclick={() => (activeTab = 'llm')}>
            {tr(lang, 'tabLlm')}
          </button>
          <button type="button" class:active={activeTab === 'backup'} onclick={() => (activeTab = 'backup')}>
            {tr(lang, 'tabBackup')}
          </button>
          <button type="button" class:active={activeTab === 'shortcuts'} onclick={() => (activeTab = 'shortcuts')}>
            {tr(lang, 'tabShortcuts')}
          </button>
        </nav>

        <section class="tab-panel">
          {#if activeTab === 'general'}
            <div class="form-grid">
              <label>
                <span>{tr(lang, 'languageLabel')}</span>
                <select bind:value={draft.ui_language}>
                  <option value="it">Italiano</option>
                  <option value="en">English</option>
                </select>
              </label>
              <label>
                <span>{tr(lang, 'themeLabel')}</span>
                <select bind:value={draft.ui_theme}>
                  <option value="system">{tr(lang, 'themeSystem')}</option>
                  <option value="light">{tr(lang, 'themeLight')}</option>
                  <option value="dark">{tr(lang, 'themeDark')}</option>
                </select>
              </label>
              <label class="check">
                <input type="checkbox" bind:checked={draft.viewer_open_in_separate_window} />
                <span>
                  {lang === 'en'
                    ? 'Open internal PDF/DjVu viewer in separate window'
                    : 'Apri visualizzatore PDF/DjVu interno in finestra separata'}
                </span>
              </label>
              <div class="two-col">
                <label>
                  <span>{lang === 'en' ? 'Worker threads' : 'Thread worker'}</span>
                  <input type="number" min="1" max="32" bind:value={draft.max_worker_threads} />
                </label>
                <label>
                  <span>{lang === 'en' ? 'DjVu render cache (MB)' : 'Cache pagine DjVu (MB)'}</span>
                  <input type="number" min="50" max="4096" bind:value={draft.cache_limit_mb} />
                </label>
              </div>
              <label>
                <span>
                  {lang === 'en'
                    ? 'External PDF command template ({file}, {page})'
                    : 'Comando lettore esterno PDF ({file}, {page})'}
                </span>
                <input
                  type="text"
                  placeholder="Default OS"
                  value={draft.external_pdf_command ?? ''}
                  oninput={(e) => {
                    const v = (e.currentTarget as HTMLInputElement).value.trim();
                    draft.external_pdf_command = v || null;
                  }}
                />
              </label>
              <label>
                <span>
                  {lang === 'en'
                    ? 'External DjVu command template ({file}, {page})'
                    : 'Comando lettore esterno DjVu ({file}, {page})'}
                </span>
                <input
                  type="text"
                  placeholder="Default OS / djview"
                  value={draft.external_djvu_command ?? ''}
                  oninput={(e) => {
                    const v = (e.currentTarget as HTMLInputElement).value.trim();
                    draft.external_djvu_command = v || null;
                  }}
                />
              </label>
              {#if presets.length > 0}
                <div class="presets-box">
                  <span class="sub-hdr">{lang === 'en' ? 'Quick Presets:' : 'Preset rapidi:'}</span>
                  <div class="preset-btns">
                    {#each presets as p (p.label)}
                      <button
                        type="button"
                        onclick={() => {
                          if (p.label.toLowerCase().includes('djvu')) {
                            draft.external_djvu_command = p.template;
                          } else {
                            draft.external_pdf_command = p.template;
                          }
                        }}
                      >
                        {p.label}
                      </button>
                    {/each}
                  </div>
                </div>
              {/if}
              <div>
                <button
                  type="button"
                  onclick={() => {
                    onClose();
                    onLaunchWizard();
                  }}
                >
                  {tr(lang, 'launchWizardBtn')}
                </button>
              </div>
            </div>
          {:else if activeTab === 'folders'}
            <div class="form-grid">
              <label class="check">
                <input type="checkbox" bind:checked={draft.fs_watcher_enabled} />
                <span>{tr(lang, 'fsWatcherLabel')}</span>
              </label>
              <div class="row-actions">
                <button type="button" class="primary" onclick={onAddFolder}>
                  {tr(lang, 'addFolder')}
                </button>
              </div>
              <ul class="roots-table">
                {#each roots as root (root.id)}
                  <li class:unreachable-root={!root.is_reachable}>
                    <label class="check">
                      <input
                        type="checkbox"
                        checked={root.is_enabled}
                        onchange={(e) =>
                          onToggleRoot(root.id, (e.currentTarget as HTMLInputElement).checked)}
                      />
                      <span>
                        <strong>
                          {root.label}
                          {#if !root.is_reachable}
                            <span class="root-warn-badge">{tr(lang, 'rootUnreachableBadge')}</span>
                          {/if}
                        </strong>
                        <small class="mono">{root.root_path}</small>
                      </span>
                    </label>
                    <div class="root-actions">
                      {#if onRebindRoot}
                        <button
                          type="button"
                          class="rebind-btn"
                          class:primary={!root.is_reachable}
                          title={lang === 'en'
                            ? 'Point this folder to its local path on this device without losing metadata or indices'
                            : 'Collega questa cartella al percorso locale su questo dispositivo senza perdere metadati o indici'}
                          onclick={() => onRebindRoot(root.id)}
                        >
                          {tr(lang, 'rebindFolderBtn')}
                        </button>
                      {/if}
                      <label class="cloud-excl" title={lang === 'en' ? 'Never send passages from this folder to cloud LLMs' : 'Non inviare mai i passaggi di questa cartella a LLM cloud'}>
                        <input
                          type="checkbox"
                          checked={root.exclude_from_cloud_llm}
                          onchange={(e) =>
                            onSetRootCloudExclusion?.(
                              root.id,
                              (e.currentTarget as HTMLInputElement).checked
                            )}
                        />
                        <span>{lang === 'en' ? 'Exclude cloud LLM' : 'Escludi da LLM cloud'}</span>
                      </label>
                      <button type="button" class="danger-btn" onclick={() => onDeleteRoot(root.id)}>
                        ✕
                      </button>
                    </div>
                  </li>
                {/each}
              </ul>
            </div>
          {:else if activeTab === 'ml'}
            <div class="form-grid">
              <label>
                <span>{lang === 'en' ? 'Active ML Profile' : 'Profilo ML attivo'}</span>
                <select bind:value={draft.ml_profile}>
                  <option value="light">Light (384-d, ~240 MB)</option>
                  <option value="quality">Quality (BGE-M3 1024-d, ~1.1 GB)</option>
                </select>
              </label>
              <div class="two-col">
                <label class="check">
                  <input type="checkbox" bind:checked={draft.reranker_enabled} />
                  <span>{lang === 'en' ? 'Cross-Encoder Reranking' : 'Reranking Cross-Encoder'}</span>
                </label>
                <label>
                  <span>{lang === 'en' ? 'Idle unload (sec)' : 'Scaricamento idle (sec)'}</span>
                  <input type="number" min="30" max="3600" bind:value={draft.model_idle_unload_secs} />
                </label>
              </div>
              <div class="two-col">
                <label>
                  <span>{lang === 'en' ? 'Score aggregation' : 'Aggregazione punteggio'}</span>
                  <select bind:value={draft.doc_score_aggregation}>
                    <option value="max">max</option>
                    <option value="top2_mean">top2_mean</option>
                  </select>
                </label>
                <label>
                  <span>{lang === 'en' ? 'Top-K docs' : 'Documenti Top-K'}</span>
                  <input type="number" min="0" max="200" bind:value={draft.search_top_k_docs} />
                </label>
              </div>

              {#if mlStatus}
                <div class="status-box">
                  <div>
                    ONNX Runtime: <strong>{mlStatus.onnxruntime_found ? '✓' : '✕'}</strong>
                    · RSS: <strong>{mlStatus.rss_mb.toFixed(1)} MB</strong>
                  </div>
                  {#each mlStatus.profiles as prof (prof.profile)}
                    <div class="prof-row">
                      <span>
                        <strong>{prof.profile}</strong>: {prof.ready ? '✓ Pronto' : 'Non scaricato'}
                      </span>
                      <button
                        type="button"
                        disabled={downloading !== null || prof.ready || !mlStatus.onnxruntime_found}
                        onclick={() => handleDownload(prof.profile as 'light' | 'quality')}
                      >
                        {downloading === prof.profile ? '…' : prof.ready ? 'Pronto' : 'Scarica'}
                      </button>
                    </div>
                  {/each}
                  <div>
                    <button type="button" onclick={onUnloadMlModels}>
                      {lang === 'en' ? 'Unload models now' : 'Scarica modelli dalla RAM ora'}
                    </button>
                  </div>
                </div>
              {/if}
              {#if mlDownloadProgress && !mlDownloadProgress.finished}
                <div class="notice">
                  {mlDownloadProgress.artifact_name}: {Math.round(
                    (mlDownloadProgress.profile_bytes_done /
                      Math.max(mlDownloadProgress.profile_bytes_total, 1)) *
                      100
                  )}%
                </div>
              {/if}
              {#if mlError}
                <div class="err-msg">{mlError}</div>
              {/if}
            </div>
          {:else if activeTab === 'ocr'}
            <div class="form-grid">
              {#if ocrStatus}
                <div class="status-box">
                  <div>
                    Tesseract: <strong>{ocrStatus.available ? '✓ Disponibile' : '✕ Non trovato'}</strong>
                    {#if ocrStatus.version}
                      <span class="mono"> · {ocrStatus.version}</span>
                    {/if}
                  </div>
                  {#if ocrStatus.available}
                    <div>
                      {lang === 'en' ? 'Selected languages' : 'Lingue attive'}:
                      <strong>{ocrStatus.selected_langs.join('+') || 'default'}</strong>
                      ({ocrStatus.installed_langs.length} {lang === 'en' ? 'installed' : 'installate'})
                    </div>
                  {:else if ocrStatus.reason}
                    <div class="err-msg">{ocrStatus.reason}</div>
                  {/if}
                </div>
              {/if}
              <label>
                <span>{lang === 'en' ? 'OCR languages (e.g. eng+ita+fra+deu)' : 'Lingue OCR (es. eng+ita+fra+deu)'}</span>
                <input
                  type="text"
                  placeholder="auto (eng+ita+fra+deu)"
                  value={draft.ocr_languages ?? ''}
                  oninput={(e) =>
                    (draft.ocr_languages = (e.currentTarget as HTMLInputElement).value.trim() || null)}
                />
              </label>
              <div class="two-col">
                <label>
                  <span>{lang === 'en' ? 'Render DPI (150-350)' : 'DPI rasterizzazione (150-350)'}</span>
                  <input type="number" min="150" max="350" bind:value={draft.ocr_dpi} />
                </label>
                <label>
                  <span>{lang === 'en' ? 'Custom tesseract binary (optional)' : 'Percorso tesseract (opzionale)'}</span>
                  <input
                    type="text"
                    placeholder="auto"
                    value={draft.tesseract_bin ?? ''}
                    oninput={(e) =>
                      (draft.tesseract_bin = (e.currentTarget as HTMLInputElement).value.trim() || null)}
                  />
                </label>
              </div>
              {#if onRunPendingOcr}
                <div class="row-actions">
                  <button
                    type="button"
                    class="primary"
                    disabled={!ocrStatus?.available}
                    onclick={() => onRunPendingOcr()}
                  >
                    {lang === 'en'
                      ? 'Run OCR on scanned files'
                      : 'Esegui OCR sui file scansionati'}
                  </button>
                </div>
              {/if}
              <p class="hint">
                {lang === 'en'
                  ? 'Original PDF/DjVu files in your library folders are strictly read-only. OCR saves extracted text into the local SQLite database & BM25 index.'
                  : 'I file PDF/DjVu nelle tue cartelle restano in sola lettura: il testo OCR viene salvato esclusivamente nel database SQLite e negli indici locali.'}
              </p>
            </div>
          {:else if activeTab === 'metadata'}
            <div class="form-grid">
              <label>
                <span>Email Polite Pool (Crossref / OpenAlex / Open Library)</span>
                <input type="email" placeholder="researcher@example.edu" bind:value={emailInput} />
              </label>
              <label class="check">
                <input type="checkbox" bind:checked={draft.network_lookups_enabled} />
                <span>{lang === 'en' ? 'Enable online metadata lookups' : 'Abilita ricerche metadati online'}</span>
              </label>

              <div class="status-box">
                <strong>
                  {lang === 'en'
                    ? 'BibTeX / BibLaTeX Keys & Live Auto-Sync .bib'
                    : 'Chiavi BibTeX / BibLaTeX & Auto-Sync .bib'}
                </strong>
                <div class="two-col">
                  <label>
                    <span>{lang === 'en' ? 'Citation key pattern' : 'Pattern Citation Key'}</span>
                    <select
                      value={draft.bibtex_key_pattern ?? 'mr_default'}
                      onchange={(e) =>
                        (draft.bibtex_key_pattern = (e.currentTarget as HTMLSelectElement)
                          .value as BibtexKeyPattern)}
                    >
                      <option value="mr_default">MR (predefinito)</option>
                      <option value="author_year">CognomeAnno</option>
                      <option value="author_year_word">CognomeAnnoParola</option>
                      <option value="author_lower_year">cognomeanno</option>
                    </select>
                  </label>
                  <label>
                    <span>{lang === 'en' ? 'Export dialect' : 'Dialetto export'}</span>
                    <select
                      value={draft.bibtex_format ?? 'bibtex'}
                      onchange={(e) =>
                        (draft.bibtex_format = (e.currentTarget as HTMLSelectElement)
                          .value as BibtexFormat)}
                    >
                      <option value="bibtex">BibTeX (AMS / MRLookup)</option>
                      <option value="biblatex">BibLaTeX (journaltitle / date)</option>
                    </select>
                  </label>
                </div>
                <div class="sub-hdr">
                  {lang === 'en' ? 'Key example:' : 'Esempio chiave:'}
                  <code class="mono">\cite{'{'}{citeKeyPreview(draft.bibtex_key_pattern)}{'}'}</code>
                </div>
                <label class="check">
                  <input
                    type="checkbox"
                    checked={Boolean(draft.bibtex_auto_sync_enabled)}
                    onchange={(e) =>
                      (draft.bibtex_auto_sync_enabled = (e.currentTarget as HTMLInputElement).checked)}
                  />
                  <span>
                    {lang === 'en'
                      ? 'Auto-sync external .bib file on every metadata edit/verification'
                      : 'Sincronizza automaticamente un file .bib esterno a ogni modifica/verifica'}
                  </span>
                </label>
                <div class="key-row">
                  <input
                    type="text"
                    readonly
                    value={draft.bibtex_auto_sync_path ?? ''}
                    placeholder={lang === 'en'
                      ? 'Choose an external .bib path (outside source folders)…'
                      : 'Scegli un file .bib esterno (fuori dalle cartelle sorgente)…'}
                  />
                  <button type="button" onclick={handleChooseBibSyncPath}>
                    {lang === 'en' ? 'Choose…' : 'Scegli…'}
                  </button>
                  {#if draft.bibtex_auto_sync_path}
                    <button
                      type="button"
                      disabled={bibSyncBusy}
                      onclick={handleSyncBibNow}
                    >
                      {bibSyncBusy ? '…' : lang === 'en' ? 'Sync now' : 'Sincronizza'}
                    </button>
                    <button
                      type="button"
                      onclick={() => {
                        draft.bibtex_auto_sync_path = null;
                        draft.bibtex_auto_sync_enabled = false;
                      }}
                    >
                      ✕
                    </button>
                  {/if}
                </div>
                {#if bibSyncMsg}
                  <div class="notice">{bibSyncMsg}</div>
                {/if}
              </div>

              <div class="prio-header">
                <div>
                  <strong>
                    {lang === 'en'
                      ? 'Search Priority & Active Sources'
                      : 'Ordine di priorità e attivazione fonti'}
                  </strong>
                  <p class="prio-sub">
                    {lang === 'en'
                      ? 'Papero queries enabled sources in exact top-to-bottom order and stops as soon as a confident match is found (drag rows or use arrows to reorder).'
                      : 'Papero interroga le fonti attive esattamente dall\'alto verso il basso: appena trova una corrispondenza certa si ferma, altrimenti passa alla successiva (trascina le righe o usa le frecce).'}
                  </p>
                </div>
                <button
                  type="button"
                  class="prio-reset-btn"
                  onclick={resetSourcePriority}
                  title={lang === 'en' ? 'Restore default source order' : 'Ripristina ordine predefinito'}
                >
                  ↺ {lang === 'en' ? 'Default' : 'Predefinito'}
                </button>
              </div>

              <ul class="prio-list">
                {#each draft.metadata_source_priority as srcId, idx (srcId)}
                  {@const info = SOURCE_META[srcId]}
                  {#if info}
                    <li
                      class="prio-item"
                      class:inactive={!isSourceActive(srcId)}
                      class:drag-over={dragOverIdx === idx && dragSourceIdx !== idx}
                      draggable="true"
                      ondragstart={() => (dragSourceIdx = idx)}
                      ondragover={(e) => {
                        e.preventDefault();
                        dragOverIdx = idx;
                      }}
                      ondragleave={() => {
                        if (dragOverIdx === idx) dragOverIdx = null;
                      }}
                      ondrop={(e) => {
                        e.preventDefault();
                        handleSourceDrop(idx);
                      }}
                      ondragend={() => {
                        dragSourceIdx = null;
                        dragOverIdx = null;
                      }}
                    >
                      <span class="drag-grip" title={lang === 'en' ? 'Drag to reorder' : 'Trascina per riordinare'}>⋮⋮</span>
                      <span class="prio-rank">#{idx + 1}</span>

                      <div class="prio-main">
                        {#if srcId === 'mrlookup'}
                          <div class="prio-title-row">
                            <strong>{info.label}</strong>
                            <div class="mr-toggles">
                              <label class="mini-check">
                                <input type="checkbox" bind:checked={draft.mrlookup_enabled_single} />
                                <span>{lang === 'en' ? 'Single' : 'Singolo'}</span>
                              </label>
                              <label
                                class="mini-check"
                                title={lang === 'en'
                                  ? 'Also use MRLookup during bulk scans (rate-limited to 1 req / 5s)'
                                  : 'Usa MRLookup anche nella scansione massiva di tutti i documenti (1 rich./5s)'}
                              >
                                <input type="checkbox" bind:checked={draft.mrlookup_enabled_bulk} />
                                <span>{lang === 'en' ? 'Bulk scan' : 'Scansione massiva'}</span>
                              </label>
                            </div>
                          </div>
                        {:else if info.toggleKey}
                          <label class="prio-check">
                            <input type="checkbox" bind:checked={draft[info.toggleKey]} />
                            <strong>{info.label}</strong>
                          </label>
                        {/if}
                        <small class="prio-desc">{lang === 'en' ? info.descEn : info.descIt}</small>
                      </div>

                      <div class="prio-btns">
                        <button
                          type="button"
                          class="prio-move-btn"
                          disabled={idx === 0}
                          onclick={() => moveSourceToTop(idx)}
                          title={lang === 'en' ? 'Move to #1' : 'Porta al 1° posto'}
                          aria-label={lang === 'en' ? 'Move to top' : 'Porta al primo posto'}
                        >
                          ⇈
                        </button>
                        <button
                          type="button"
                          class="prio-move-btn"
                          disabled={idx === 0}
                          onclick={() => moveSourcePriority(idx, -1)}
                          title={lang === 'en' ? 'Move up' : 'Alza priorità'}
                          aria-label={lang === 'en' ? 'Move up' : 'Alza priorità'}
                        >
                          ↑
                        </button>
                        <button
                          type="button"
                          class="prio-move-btn"
                          disabled={idx === draft.metadata_source_priority.length - 1}
                          onclick={() => moveSourcePriority(idx, 1)}
                          title={lang === 'en' ? 'Move down' : 'Abbassa priorità'}
                          aria-label={lang === 'en' ? 'Move down' : 'Abbassa priorità'}
                        >
                          ↓
                        </button>
                      </div>
                    </li>
                  {/if}
                {/each}
              </ul>
            </div>
          {:else if activeTab === 'llm'}
            <div class="form-grid">
              <div class="llm-callout">
                <strong>
                  {lang === 'en'
                    ? 'Optional & On-Demand Only (Never Automatic)'
                    : 'Funzione opzionale e solo su richiesta (mai automatica)'}
                </strong>
                <p>
                  {lang === 'en'
                    ? 'Consumer web plans (ChatGPT Plus, Claude Pro, Gemini Advanced) do NOT include API access. Generate an API key in the developer console (usage-based or provider free tier) or use local Ollama 100% offline.'
                    : 'Gli abbonamenti web consumer (ChatGPT Plus, Claude Pro, Gemini Advanced) NON includono l\'accesso API. Serve una API key dal portale sviluppatori (a consumo / eventuale piano gratuito) oppure Ollama locale 100% offline.'}
                </p>
                <div class="llm-links">
                  <a href="https://console.anthropic.com/settings/keys" target="_blank" rel="noopener noreferrer">Anthropic Console ↗</a>
                  <a href="https://platform.openai.com/api-keys" target="_blank" rel="noopener noreferrer">OpenAI API Keys ↗</a>
                  <a href="https://aistudio.google.com/apikey" target="_blank" rel="noopener noreferrer">Google AI Studio ↗</a>
                </div>
              </div>

              <label>
                <span>{lang === 'en' ? 'Active LLM Provider' : 'Provider LLM attivo'}</span>
                <select
                  value={draft.llm_provider}
                  onchange={(e) =>
                    handleProviderChange((e.currentTarget as HTMLSelectElement).value as LlmProvider)}
                >
                  <option value="disabled">
                    {lang === 'en' ? 'Disabled (default)' : 'Disattivato (predefinito)'}
                  </option>
                  <option value="ollama">
                    {lang === 'en' ? 'Ollama (Local — 100% offline)' : 'Ollama (Locale — 100% offline)'}
                  </option>
                  <option value="anthropic">Anthropic (Claude API)</option>
                  <option value="openai">OpenAI API</option>
                  <option value="gemini">Google Gemini API</option>
                </select>
              </label>

              {#if draft.llm_provider === 'ollama'}
                <label>
                  <span>{lang === 'en' ? 'Local Ollama URL' : 'URL Ollama locale'}</span>
                  <div class="key-row">
                    <input
                      type="text"
                      bind:value={draft.llm_ollama_url}
                      placeholder="http://127.0.0.1:11434"
                    />
                    <button type="button" disabled={llmBusy} onclick={handleVerifyLlmKey}>
                      {lang === 'en' ? 'Check & load models' : 'Verifica e carica modelli'}
                    </button>
                  </div>
                </label>
              {:else if isCloudProvider(draft.llm_provider)}
                <div class="status-box">
                  <div class="prof-row">
                    <span>
                      {lang === 'en' ? 'OS Keychain status:' : 'Stato Portachiavi di sistema:'}
                    </span>
                    <span class={hasKeyForProvider(draft.llm_provider) ? 'badge-ok' : 'badge-warn'}>
                      {hasKeyForProvider(draft.llm_provider)
                        ? lang === 'en'
                          ? 'Key saved (org.papero.app)'
                          : 'Chiave salvata (org.papero.app)'
                        : lang === 'en'
                          ? 'No key stored'
                          : 'Nessuna chiave salvata'}
                    </span>
                  </div>
                  <div class="key-row">
                    <input
                      type="password"
                      autocomplete="off"
                      bind:value={llmKeyInput}
                      placeholder={lang === 'en'
                        ? 'Paste API key (stored only in OS Keychain)…'
                        : 'Incolla la API key (salvata solo nel Portachiavi di sistema)…'}
                    />
                    <button
                      type="button"
                      disabled={llmBusy || !llmKeyInput.trim()}
                      onclick={handleSaveLlmKey}
                    >
                      {lang === 'en' ? 'Save key' : 'Salva chiave'}
                    </button>
                  </div>
                  <div class="row-actions">
                    <button type="button" disabled={llmBusy} onclick={handleVerifyLlmKey}>
                      {lang === 'en' ? 'Verify key' : 'Verifica chiave'}
                    </button>
                    {#if hasKeyForProvider(draft.llm_provider)}
                      <button
                        type="button"
                        class="danger-btn"
                        disabled={llmBusy}
                        onclick={handleRemoveLlmKey}
                      >
                        {lang === 'en' ? 'Remove key' : 'Rimuovi chiave'}
                      </button>
                    {/if}
                  </div>
                  {#if hasConsentForProvider(draft.llm_provider)}
                    <div class="consent-row">
                      <small>
                        {lang === 'en'
                          ? 'First-send cloud privacy consent accepted.'
                          : 'Consenso al primo invio cloud già accettato.'}
                      </small>
                      <button
                        type="button"
                        onclick={() => revokeConsentForProvider(draft.llm_provider)}
                      >
                        {lang === 'en' ? 'Reset consent' : 'Revoca consenso'}
                      </button>
                    </div>
                  {/if}
                  {#if draft.llm_provider === 'gemini'}
                    <p class="warn-callout">
                      {lang === 'en'
                        ? 'Google Gemini privacy notice: on Gemini API Free Tier (Unpaid), prompts and responses may be reviewed by humans and used to improve Google products. On a Paid billing account, prompts are not used for product improvement.'
                        : 'Nota privacy Google Gemini: nel piano gratuito (Unpaid), Google può usare e revisionare prompt e risposte per migliorare i propri prodotti. Con fatturazione attiva (Paid Tier) gli input non sono usati per addestramento.'}
                    </p>
                  {:else}
                    <p class="hint">
                      {lang === 'en'
                        ? 'Commercial API terms: inputs sent via API are not used for model training by default. API keys are NEVER written to config files, logs, or backups.'
                        : 'Policy API commerciali: gli input inviati via API non sono usati per addestramento per impostazione predefinita. Le chiavi NON vengono mai salvate in config, log o backup.'}
                    </p>
                  {/if}
                </div>
              {/if}

              {#if draft.llm_provider !== 'disabled'}
                <div class="form-grid">
                  <label>
                    <span>{lang === 'en' ? 'Model (live from provider)' : 'Modello (lista live dal provider)'}</span>
                    <div class="key-row">
                      {#if llmModels.length > 0}
                        <select bind:value={draft.llm_model}>
                          {#each llmModels as m (m.id)}
                            <option value={m.id}>{m.display_name}</option>
                          {/each}
                        </select>
                      {:else}
                        <input
                          type="text"
                          readonly
                          value={draft.llm_model ?? ''}
                          placeholder={lang === 'en'
                            ? 'Click "Refresh models" to fetch live models…'
                            : 'Clicca «Aggiorna modelli» per caricare i modelli reali…'}
                        />
                      {/if}
                      <button type="button" disabled={llmBusy} onclick={handleFetchLlmModels}>
                        {lang === 'en' ? 'Refresh models' : 'Aggiorna modelli'}
                      </button>
                    </div>
                  </label>

                  <div class="two-col">
                    <label>
                      <span>{lang === 'en' ? 'Max passages (1-40)' : 'Max passaggi (1-40)'}</span>
                      <input type="number" min="1" max="40" bind:value={draft.llm_max_passages} />
                    </label>
                    <label>
                      <span>{lang === 'en' ? 'Max input tokens' : 'Max token input'}</span>
                      <input
                        type="number"
                        min="256"
                        max="64000"
                        step="100"
                        bind:value={draft.llm_max_input_tokens}
                      />
                    </label>
                    <label>
                      <span>{lang === 'en' ? 'Max output tokens' : 'Max token output'}</span>
                      <input
                        type="number"
                        min="128"
                        max="8192"
                        step="50"
                        bind:value={draft.llm_max_output_tokens}
                      />
                    </label>
                  </div>

                  {#if llmMsg}
                    <div class="notice">{llmMsg}</div>
                  {/if}
                  {#if llmErr}
                    <div class="err-msg">{llmErr}</div>
                  {/if}
                  <p class="hint">
                    {lang === 'en'
                      ? 'Click "Save Settings" below after choosing a provider/model or changing limits.'
                      : 'Premi «Salva impostazioni» in basso dopo aver scelto provider/modello o limiti.'}
                  </p>
                </div>
              {/if}
            </div>
          {:else if activeTab === 'backup'}
            <div class="form-grid">
              <p class="hint">{tr(lang, 'backupDesc')}</p>
              <div class="status-box">
                <div class="prof-row">
                  <strong>{tr(lang, 'backupPathLabel')}</strong>
                  {#if settings.backup_path}
                    <button
                      type="button"
                      disabled={backupBusy}
                      onclick={async () => {
                        draft.backup_path = null;
                        await onSaveSettings({
                          ...settings,
                          backup_path: null,
                          auto_backup_enabled: draft.auto_backup_enabled ?? true,
                          polite_email: emailInput.trim() || null,
                        });
                      }}
                    >
                      {tr(lang, 'clearBackupPathBtn')}
                    </button>
                  {/if}
                </div>
                {#if settings.backup_path}
                  <div class="mono" style="word-break: break-all;">{settings.backup_path}</div>
                  {#if settings.last_backup_at}
                    <div class="sub-hdr">
                      {tr(lang, 'lastBackupLabel')}:
                      {new Date(settings.last_backup_at * 1000).toLocaleString()}
                      {#if settings.last_backup_bytes}
                        · {settings.last_backup_bytes >= 1024 * 1024
                          ? `${(settings.last_backup_bytes / (1024 * 1024)).toFixed(1)} MB`
                          : `${Math.max(1, Math.round(settings.last_backup_bytes / 1024))} KB`}
                      {/if}
                    </div>
                  {/if}
                {:else}
                  <div class="sub-hdr">{tr(lang, 'noBackupYet')}</div>
                {/if}
              </div>

              <label class="check">
                <input
                  type="checkbox"
                  checked={draft.auto_backup_enabled ?? true}
                  onchange={async (e) => {
                    const checked = (e.currentTarget as HTMLInputElement).checked;
                    draft.auto_backup_enabled = checked;
                    await onSaveSettings({
                      ...settings,
                      auto_backup_enabled: checked,
                      polite_email: emailInput.trim() || null,
                    });
                  }}
                />
                <span>{tr(lang, 'autoBackupLabel')}</span>
              </label>
              <p class="hint" style="font-size: 11px;">{tr(lang, 'autoBackupHint')}</p>

              <div class="row-actions">
                <button type="button" class="primary" disabled={backupBusy} onclick={onCreateBackup}>
                  {tr(lang, settings.backup_path ? 'createNewBackupBtn' : 'createBackupBtn')}
                </button>
                {#if settings.backup_path && onUpdateBackupNow}
                  <button type="button" disabled={backupBusy} onclick={onUpdateBackupNow}>
                    {tr(lang, 'updateBackupNowBtn')}
                  </button>
                {/if}
                <button type="button" disabled={backupBusy} onclick={onRestoreBackup}>
                  {tr(lang, 'restoreBackupBtn')}
                </button>
              </div>

              {#if backupBusy}
                <div class="notice">{tr(lang, 'backupRunningMsg')}</div>
              {/if}
              {#if backupNotice}
                <div class="notice">{backupNotice}</div>
              {/if}
              {#if backupError}
                <div class="err-msg">{backupError}</div>
              {/if}
            </div>
          {:else}
            <div class="shortcuts-list">
              <div class="sc-row"><kbd>/</kbd> <kbd>Cmd/Ctrl + K</kbd> <span>{lang === 'en' ? 'Focus search bar' : 'Focus sulla barra di ricerca'}</span></div>
              <div class="sc-row"><kbd>Cmd/Ctrl + F</kbd> <span>{lang === 'en' ? 'Find inside open PDF / DjVu' : 'Trova nel documento aperto (PDF / DjVu)'}</span></div>
              <div class="sc-row"><kbd>Alt + ← / →</kbd> <span>{lang === 'en' ? 'Back / Forward jump history in viewer' : 'Indietro / Avanti nella cronologia di salto del viewer'}</span></div>
              <div class="sc-row"><kbd>Cmd/Ctrl + S</kbd> <span>{lang === 'en' ? 'Save metadata changes in Inspector' : 'Salva modifiche metadati nell’Ispettore'}</span></div>
              <div class="sc-row"><kbd>Shift / Cmd + Click</kbd> <span>{lang === 'en' ? 'Multi-select documents for batch actions' : 'Selezione multipla documenti per azioni batch'}</span></div>
              <div class="sc-row"><kbd>Cmd/Ctrl + B</kbd> <span>{lang === 'en' ? 'Toggle left sidebar' : 'Mostra/nascondi barra laterale sinistra'}</span></div>
              <div class="sc-row"><kbd>Cmd/Ctrl + R</kbd> <span>{lang === 'en' ? 'Incremental rescan' : 'Riscansione incrementale cartelle'}</span></div>
              <div class="sc-row"><kbd>Cmd/Ctrl + ,</kbd> <span>{lang === 'en' ? 'Open settings' : 'Apri Impostazioni'}</span></div>
              <div class="sc-row"><kbd>Esc</kbd> <span>{lang === 'en' ? 'Close modal / Find bar / popover' : 'Chiudi modale / barra Trova / popover'}</span></div>
            </div>
          {/if}
        </section>
      </div>

      <footer class="modal-foot">
        <span class="foot-status">{statusNotice}</span>
        <div class="foot-btns">
          <button type="button" onclick={onClose}>{tr(lang, 'close')}</button>
          <button type="button" class="primary" disabled={saving} onclick={saveDraft}>
            {tr(lang, 'saveSettings')}
          </button>
        </div>
      </footer>
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(15, 23, 42, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 55;
    padding: 16px;
  }
  .settings-modal {
    width: min(760px, 100%);
    height: min(560px, 90vh);
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 20px 55px rgba(0, 0, 0, 0.35);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .modal-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
  }
  .modal-head h2 {
    margin: 0;
    font-size: 16px;
  }
  .modal-body {
    flex: 1;
    display: grid;
    grid-template-columns: 185px 1fr;
    overflow: hidden;
  }
  .tab-nav {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 10px;
    background: var(--bg-muted);
    border-right: 1px solid var(--border);
  }
  .tab-nav button {
    text-align: left;
    background: transparent;
    border: 1px solid transparent;
  }
  .tab-nav button.active {
    background: var(--accent-soft);
    color: var(--accent);
    border-color: var(--accent);
    font-weight: 600;
  }
  .tab-panel {
    padding: 14px 16px;
    overflow-y: auto;
  }
  .form-grid {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .form-grid label:not(.check) {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .form-grid label span {
    font-size: 12px;
    color: var(--text-muted);
  }
  label.check {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
  }
  .two-col {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .presets-box {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .sub-hdr {
    font-size: 11px;
    color: var(--text-muted);
  }
  .preset-btns,
  .row-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .roots-table {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .roots-table li {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .roots-table li.unreachable-root {
    border-color: rgba(217, 119, 6, 0.55);
    background: rgba(245, 158, 11, 0.08);
  }
  .roots-table li span {
    display: flex;
    flex-direction: column;
  }
  .root-warn-badge {
    display: inline !important;
    margin-left: 6px;
    padding: 1px 6px;
    border-radius: 4px;
    font-size: 10.5px;
    font-weight: 600;
    background: rgba(217, 119, 6, 0.16);
    color: var(--warn);
  }
  .rebind-btn {
    font-size: 11px;
    padding: 3px 8px;
    white-space: nowrap;
  }
  .root-actions {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-shrink: 0;
  }
  .cloud-excl {
    display: flex;
    flex-direction: row !important;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    color: var(--text-muted);
    white-space: nowrap;
  }
  .cloud-excl span {
    display: inline !important;
  }
  .llm-callout {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 10px;
    background: var(--bg-muted);
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-size: 12px;
  }
  .llm-callout p {
    margin: 0;
    color: var(--text-muted);
    line-height: 1.45;
  }
  .llm-links {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
    font-size: 11px;
  }
  .llm-links a {
    color: var(--accent);
    text-decoration: none;
  }
  .llm-links a:hover {
    text-decoration: underline;
  }
  .key-row {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .key-row input,
  .key-row select {
    flex: 1;
    min-width: 0;
  }
  .badge-ok {
    font-size: 11px;
    font-weight: 600;
    color: #16a34a;
  }
  .badge-warn {
    font-size: 11px;
    color: var(--text-muted);
  }
  .consent-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 11px;
    color: var(--text-muted);
  }
  .warn-callout {
    margin: 0;
    padding: 6px 8px;
    border-radius: 6px;
    background: rgba(245, 158, 11, 0.14);
    border: 1px solid rgba(245, 158, 11, 0.45);
    color: var(--text);
    font-size: 11px;
    line-height: 1.4;
  }
  .mono {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    color: var(--text-muted);
  }
  .danger-btn {
    color: var(--danger);
  }
  .status-box {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: var(--bg-muted);
  }
  .prof-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .hint {
    margin: 0;
    color: var(--text-muted);
    line-height: 1.5;
  }
  .notice {
    padding: 6px 8px;
    border-radius: 6px;
    background: var(--accent-soft);
    font-size: 12px;
  }
  .err-msg {
    color: var(--danger);
    font-size: 12px;
  }
  .shortcuts-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .sc-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .sc-row kbd {
    background: var(--bg-muted);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 2px 6px;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
  }
  .modal-foot {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 16px;
    border-top: 1px solid var(--border);
  }
  .foot-status {
    font-size: 12px;
    color: var(--accent);
  }
  .foot-btns {
    display: flex;
    gap: 8px;
  }
  .prio-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 10px;
    padding-top: 4px;
    border-top: 1px solid var(--border);
  }
  .prio-header strong {
    font-size: 12.5px;
  }
  .prio-sub {
    margin: 2px 0 0;
    font-size: 11px;
    color: var(--text-muted);
    line-height: 1.4;
  }
  .prio-reset-btn {
    white-space: nowrap;
    font-size: 11px;
    padding: 3px 8px;
    flex-shrink: 0;
  }
  .prio-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .prio-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--bg-panel);
    transition: border-color 0.12s, opacity 0.12s;
  }
  .prio-item.inactive {
    opacity: 0.55;
    background: var(--bg-muted);
  }
  .prio-item.drag-over {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .drag-grip {
    cursor: grab;
    color: var(--text-muted);
    font-size: 12px;
    user-select: none;
    padding: 0 2px;
  }
  .prio-rank {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    font-weight: 700;
    color: var(--accent);
    min-width: 24px;
    text-align: center;
    background: var(--accent-soft);
    border-radius: 4px;
    padding: 2px 4px;
  }
  .prio-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .prio-check {
    display: inline-flex !important;
    flex-direction: row !important;
    align-items: center;
    gap: 6px !important;
    cursor: pointer;
    font-size: 12px;
  }
  .prio-title-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 12px;
  }
  .mr-toggles {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .mini-check {
    display: inline-flex !important;
    flex-direction: row !important;
    align-items: center;
    gap: 4px !important;
    font-size: 11px;
    cursor: pointer;
  }
  .mini-check span {
    font-size: 11px !important;
    color: var(--text) !important;
  }
  .prio-desc {
    font-size: 11px;
    color: var(--text-muted);
  }
  .prio-btns {
    display: flex;
    align-items: center;
    gap: 3px;
    flex-shrink: 0;
  }
  .prio-move-btn {
    padding: 2px 6px;
    font-size: 11px;
    line-height: 1.2;
  }
</style>
