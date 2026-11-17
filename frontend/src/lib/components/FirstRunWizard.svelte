<script lang="ts">
  import type {
    DownloadProgressEvent,
    MlEnvironmentStatus,
    SourceRoot,
    UiSettingsPayload,
  } from '../types';
  import { normalizeLang, tr } from '../i18n';

  interface Props {
    open: boolean;
    settings: UiSettingsPayload;
    roots: SourceRoot[];
    mlStatus: MlEnvironmentStatus | null;
    mlDownloadProgress: DownloadProgressEvent | null;
    onAddFolder: () => Promise<void>;
    onRestoreBackup?: () => Promise<void>;
    backupBusy?: boolean;
    backupNotice?: string;
    backupError?: string;
    onDownloadMlProfile: (profile: 'light' | 'quality') => Promise<void>;
    onSaveSettings: (next: UiSettingsPayload) => Promise<void>;
    onCompleteAndScan: () => Promise<void>;
    onClose: () => void;
  }

  let {
    open,
    settings,
    roots,
    mlStatus,
    mlDownloadProgress,
    onAddFolder,
    onRestoreBackup,
    backupBusy = false,
    backupNotice = '',
    backupError = '',
    onDownloadMlProfile,
    onSaveSettings,
    onCompleteAndScan,
    onClose,
  }: Props = $props();

  let step = $state<1 | 2 | 3 | 4>(1);
  let selectedProfile = $state<'light' | 'quality'>('light');
  let politeEmail = $state('');
  let selectedLang = $state<'it' | 'en'>('it');
  let downloading = $state(false);
  let downloadErr = $state('');
  let finishing = $state(false);

  $effect(() => {
    if (open && settings) {
      selectedProfile = settings.ml_profile === 'quality' ? 'quality' : 'light';
      politeEmail = settings.polite_email ?? '';
      selectedLang = normalizeLang(settings.ui_language);
      downloadErr = '';
    }
  });

  let lightInfo = $derived(mlStatus?.profiles.find((p) => p.profile === 'light'));
  let qualityInfo = $derived(mlStatus?.profiles.find((p) => p.profile === 'quality'));
  let chosenInfo = $derived(selectedProfile === 'quality' ? qualityInfo : lightInfo);

  async function handleDownloadSelected() {
    downloading = true;
    downloadErr = '';
    try {
      await onSaveSettings({ ...settings, ml_profile: selectedProfile, ui_language: selectedLang });
      await onDownloadMlProfile(selectedProfile);
    } catch (e) {
      downloadErr = String(e);
    } finally {
      downloading = false;
    }
  }

  async function handleFinish(startScan: boolean) {
    finishing = true;
    try {
      await onSaveSettings({
        ...settings,
        ml_profile: selectedProfile,
        polite_email: politeEmail.trim() || null,
        ui_language: selectedLang,
        first_run_completed: true,
      });
      if (startScan && roots.length > 0) {
        await onCompleteAndScan();
      } else {
        onClose();
      }
    } finally {
      finishing = false;
    }
  }
</script>

{#if open}
  <div class="wizard-backdrop" role="dialog" aria-modal="true" aria-label={tr(selectedLang, 'wizardTitle')}>
    <div class="wizard-card">
      <div class="wiz-head">
        <div>
          <h2>{tr(selectedLang, 'wizardTitle')}</h2>
          <p class="wiz-sub">
            {selectedLang === 'en'
              ? `Step ${step} of 4 — Local setup (zero cloud cost, read-only source files)`
              : `Passo ${step} di 4 — Configurazione locale (zero costi, file originali in sola lettura)`}
          </p>
        </div>
        <div class="lang-switch">
          <button
            type="button"
            class:active={selectedLang === 'it'}
            onclick={async () => {
              selectedLang = 'it';
              await onSaveSettings({ ...settings, ui_language: 'it' });
            }}
          >
            IT
          </button>
          <button
            type="button"
            class:active={selectedLang === 'en'}
            onclick={async () => {
              selectedLang = 'en';
              await onSaveSettings({ ...settings, ui_language: 'en' });
            }}
          >
            EN
          </button>
        </div>
      </div>

      <div class="steps-bar">
        {#each [1, 2, 3, 4] as s}
          <div class="step-pill" class:active={step === s} class:done={step > s}>{s}</div>
        {/each}
      </div>

      <div class="wiz-body">
        {#if step === 1}
          <h3>
            {selectedLang === 'en'
              ? '1. Source Document Folders (PDF / DjVu)'
              : '1. Cartelle sorgente dei documenti (PDF / DjVu)'}
          </h3>
          <p class="hint-text">
            {selectedLang === 'en'
              ? 'Papero indexes your folders strictly read-only: it never writes sidecar files or renames inside your folders.'
              : 'Papero indicizza le cartelle in rigorosa sola lettura: non scrive né sposta mai nulla nelle tue cartelle originali.'}
          </p>
          <div class="action-row">
            <button type="button" class="primary" onclick={onAddFolder}>
              {selectedLang === 'en' ? '+ Add folder…' : '+ Aggiungi cartella…'}
            </button>
            {#if onRestoreBackup}
              <button type="button" disabled={backupBusy} onclick={onRestoreBackup}>
                {tr(selectedLang, 'wizardRestoreBackupBtn')}
              </button>
            {/if}
          </div>
          {#if backupNotice}
            <p class="prog-txt">{backupNotice}</p>
          {/if}
          {#if backupError}
            <p class="err-txt">{backupError}</p>
          {/if}
          {#if roots.length === 0}
            <p class="empty-msg">
              {selectedLang === 'en'
                ? 'No folders added yet (you can also add them later).'
                : 'Nessuna cartella aggiunta (puoi aggiungerle anche in seguito).'}
            </p>
          {:else}
            <ul class="root-list">
              {#each roots as r (r.id)}
                <li>
                  <strong>{r.label}</strong>
                  <span class="mono">{r.root_path}</span>
                </li>
              {/each}
            </ul>
          {/if}
        {:else if step === 2}
          <h3>
            {selectedLang === 'en'
              ? '2. Local Semantic Search Models (Optional Download)'
              : '2. Modelli locali per ricerca semantica (download opzionale)'}
          </h3>
          <p class="hint-text">
            {selectedLang === 'en'
              ? 'Keyword/BM25 search works immediately without downloads. ONNX models enable hybrid vector + cross-encoder reranking.'
              : 'La ricerca testuale BM25 funziona subito senza download. I modelli ONNX abilitano ricerca semantica ibrida e reranking locale.'}
          </p>
          <div class="profile-grid">
            <button
              type="button"
              class="prof-card"
              class:selected={selectedProfile === 'light'}
              onclick={() => (selectedProfile = 'light')}
            >
              <strong>Light (384-d, ~240 MB)</strong>
              <span>
                {selectedLang === 'en'
                  ? 'Fast on laptop CPU · Recommended default'
                  : 'Veloce su CPU di portatili · Consigliato'}
              </span>
              <small>{lightInfo?.ready ? '✓ Pronto' : 'Da scaricare'}</small>
            </button>
            <button
              type="button"
              class="prof-card"
              class:selected={selectedProfile === 'quality'}
              onclick={() => (selectedProfile = 'quality')}
            >
              <strong>Quality (BGE-M3 1024-d, ~1.1 GB)</strong>
              <span>
                {selectedLang === 'en'
                  ? 'Higher multilingual/math recall · Needs more RAM/disk'
                  : 'Massima accuratezza multilingua · Richiede più disco/RAM'}
              </span>
              <small>{qualityInfo?.ready ? '✓ Pronto' : 'Da scaricare'}</small>
            </button>
          </div>

          <div class="dl-row">
            <button
              type="button"
              class="primary"
              disabled={downloading || !!chosenInfo?.ready || !mlStatus?.onnxruntime_found}
              onclick={handleDownloadSelected}
            >
              {#if chosenInfo?.ready}
                {selectedLang === 'en' ? 'Models Ready ✓' : 'Modelli già pronti ✓'}
              {:else if downloading}
                {selectedLang === 'en' ? 'Downloading…' : 'Download in corso…'}
              {:else}
                {selectedLang === 'en'
                  ? `Download ${selectedProfile} (~${selectedProfile === 'light' ? '240 MB' : '1.1 GB'})`
                  : `Scarica profilo ${selectedProfile} (~${selectedProfile === 'light' ? '240 MB' : '1.1 GB'})`}
              {/if}
            </button>
            {#if !mlStatus?.onnxruntime_found}
              <span class="warn-txt">ONNX Runtime non rilevato nel sistema</span>
            {/if}
          </div>
          {#if mlDownloadProgress && !mlDownloadProgress.finished}
            <div class="prog-txt">
              {mlDownloadProgress.artifact_name}: {Math.round(
                (mlDownloadProgress.profile_bytes_done /
                  Math.max(mlDownloadProgress.profile_bytes_total, 1)) *
                  100
              )}%
            </div>
          {/if}
          {#if downloadErr}
            <div class="err-txt">{downloadErr}</div>
          {/if}
        {:else if step === 3}
          <h3>
            {selectedLang === 'en'
              ? '3. Optional Email for Polite Metadata Lookups'
              : '3. Email opzionale per metadati bibliografici (Polite Pool)'}
          </h3>
          <p class="hint-text">
            {selectedLang === 'en'
              ? 'Used strictly in the HTTP User-Agent/mailto header for Crossref, OpenAlex & Open Library polite rate limits, stored in your OS Keychain. Never shared elsewhere.'
              : 'Usata solo negli header HTTP di Crossref, OpenAlex e Open Library per evitare limitazioni di banda, salvata nel Portachiavi di sistema.'}
          </p>
          <label class="email-field">
            <span>Email ({selectedLang === 'en' ? 'optional' : 'opzionale'})</span>
            <input type="email" placeholder="researcher@example.edu" bind:value={politeEmail} />
          </label>
        {:else}
          <h3>
            {selectedLang === 'en' ? '4. Ready to Catalog' : '4. Tutto pronto'}
          </h3>
          <ul class="summary-list">
            <li>
              <strong>{selectedLang === 'en' ? 'Folders' : 'Cartelle'}:</strong> {roots.length}
            </li>
            <li>
              <strong>{selectedLang === 'en' ? 'ML Profile' : 'Profilo ML'}:</strong> {selectedProfile}
              ({chosenInfo?.ready ? 'pronto' : 'BM25 immediato'})
            </li>
            <li>
              <strong>Polite email:</strong> {politeEmail.trim() || '—'}
            </li>
          </ul>
          <p class="hint-text">
            {selectedLang === 'en'
              ? 'You can revisit this wizard or adjust every preference anytime in Settings.'
              : 'Puoi riaprire questa procedura guidata o modificare qualsiasi preferenza in qualsiasi momento dalle Impostazioni.'}
          </p>
        {/if}
      </div>

      <div class="wiz-foot">
        <button type="button" onclick={() => handleFinish(false)} disabled={finishing}>
          {tr(selectedLang, 'wizardSkip')}
        </button>
        <div class="foot-right">
          {#if step > 1}
            <button type="button" onclick={() => (step = (step - 1) as 1 | 2 | 3 | 4)}>
              {tr(selectedLang, 'wizardBack')}
            </button>
          {/if}
          {#if step < 4}
            <button
              type="button"
              class="primary"
              onclick={() => (step = (step + 1) as 1 | 2 | 3 | 4)}
            >
              {step === 3 && !politeEmail.trim()
                ? tr(selectedLang, 'wizardSkip')
                : tr(selectedLang, 'wizardNext')}
            </button>
          {:else}
            <button
              type="button"
              class="primary"
              disabled={finishing}
              onclick={() => handleFinish(true)}
            >
              {tr(selectedLang, 'wizardFinish')}
            </button>
          {/if}
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  .wizard-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(15, 23, 42, 0.58);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 60;
    padding: 16px;
  }
  .wizard-card {
    width: min(620px, 100%);
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 20px 50px rgba(0, 0, 0, 0.35);
    padding: 18px 20px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .wiz-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
  }
  .wiz-head h2 {
    margin: 0;
    font-size: 18px;
  }
  .wiz-sub {
    margin: 2px 0 0;
    font-size: 12px;
    color: var(--text-muted);
  }
  .lang-switch {
    display: flex;
    gap: 4px;
  }
  .lang-switch button.active {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
  }
  .steps-bar {
    display: flex;
    gap: 8px;
  }
  .step-pill {
    flex: 1;
    text-align: center;
    padding: 4px;
    border-radius: 999px;
    background: var(--bg-muted);
    color: var(--text-muted);
    font-weight: 600;
    font-size: 11px;
  }
  .step-pill.active {
    background: var(--accent);
    color: #fff;
  }
  .step-pill.done {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .wiz-body {
    min-height: 200px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .wiz-body h3 {
    margin: 0;
    font-size: 14px;
  }
  .action-row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .hint-text,
  .empty-msg {
    margin: 0;
    color: var(--text-muted);
    line-height: 1.45;
  }
  .root-list {
    margin: 0;
    padding-left: 16px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 130px;
    overflow-y: auto;
  }
  .root-list li {
    display: flex;
    flex-direction: column;
  }
  .mono {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    color: var(--text-muted);
  }
  .profile-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .prof-card {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    text-align: left;
    gap: 4px;
    padding: 10px;
    border-radius: 8px;
  }
  .prof-card.selected {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .prof-card span {
    font-size: 11.5px;
    color: var(--text-muted);
  }
  .dl-row {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .warn-txt {
    color: var(--warn);
    font-size: 11.5px;
  }
  .err-txt {
    color: var(--danger);
    font-size: 11.5px;
  }
  .prog-txt {
    font-size: 11.5px;
    color: var(--accent);
  }
  .email-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .summary-list {
    margin: 0;
    padding-left: 18px;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .wiz-foot {
    display: flex;
    justify-content: space-between;
    align-items: center;
    border-top: 1px solid var(--border);
    padding-top: 10px;
  }
  .foot-right {
    display: flex;
    gap: 8px;
  }
</style>
