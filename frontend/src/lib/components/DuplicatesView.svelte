<script lang="ts">
  import type { DuplicateFileMember, DuplicateGroupView } from '../types';

  interface Props {
    groups: DuplicateGroupView[];
    scanningDedup: boolean;
    onRunDedup: () => void;
    onMerge: (groupId: number, keepDocumentId: number) => void;
    onResolve: (groupId: number, decision: 'distinct' | 'versions' | 'editions') => void;
    onOpenSideBySide: (members: DuplicateFileMember[]) => void;
    onOpenMember: (m: DuplicateFileMember) => void;
    onRevealFile: (fileId: number) => void;
  }

  let {
    groups,
    scanningDedup,
    onRunDedup,
    onMerge,
    onResolve,
    onOpenSideBySide,
    onOpenMember,
    onRevealFile,
  }: Props = $props();

  let showResolved = $state(false);

  const visibleGroups = $derived(
    showResolved ? groups : groups.filter((g) => g.status === 'pending' || g.status === 'info_merged')
  );

  function levelLabel(lvl: number): string {
    switch (lvl) {
      case 1:
        return 'Livello 1 · Copie identiche';
      case 2:
        return 'Livello 2 · Stesso ID opera';
      case 3:
        return 'Livello 3 · Duplicato probabile';
      case 4:
        return 'Livello 4 · Preprint / Versione';
      case 5:
        return 'Livello 5 · Edizioni dello stesso libro';
      default:
        return `Livello ${lvl}`;
    }
  }

  function formatBytes(n: number): string {
    if (n >= 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
    if (n >= 1024) return `${Math.round(n / 1024)} KB`;
    return `${n} B`;
  }
</script>

<div class="dup-view">
  <div class="dup-header">
    <div>
      <strong>Doppioni &amp; Edizioni ({visibleGroups.length})</strong>
    </div>
    <div class="dup-controls">
      <label class="toggle-resolved">
        <input type="checkbox" bind:checked={showResolved} />
        Mostra risolti
      </label>
      <button type="button" class="primary" onclick={onRunDedup} disabled={scanningDedup}>
        {scanningDedup ? 'Analisi…' : 'Verifica doppioni'}
      </button>
    </div>
  </div>

  {#if visibleGroups.length === 0}
    <div class="empty">
      Nessun gruppo di duplicati o edizioni da esaminare.
    </div>
  {:else}
    <div class="groups-list">
      {#each visibleGroups as g (g.group_id)}
        <div class="group-card">
          <div class="group-head">
            <span class="level-badge">L{g.group_level}</span>
            <strong>{levelLabel(g.group_level)}</strong>
            <span class="sim">{Math.round(g.max_similarity * 100)}%</span>
            {#if g.status !== 'pending'}
              <span class="status-pill">{g.status}</span>
            {/if}
          </div>
          <div class="reason">{g.reason_detail}</div>

          <div class="members">
            {#each g.members as m (`${g.group_id}-${m.document_id}-${m.file_id}`)}
              <div class="member-row" class:unavailable={!m.is_available}>
                <div class="member-info">
                  <div class="member-title">
                    <span class="fmt">{m.format.toUpperCase()}</span>
                    <span>{m.title}</span>
                  </div>
                  <div class="member-meta">
                    <span>{m.root_label}/{m.rel_path}</span>
                    {#if m.edition}<span> · {m.edition} ed.</span>{/if}
                    {#if m.year}<span> · {m.year}</span>{/if}
                    {#if m.page_count}<span> · {m.page_count} pp.</span>{/if}
                    <span> · {formatBytes(m.size_bytes)}</span>
                  </div>
                  <div class="member-ids">
                    {#if m.doi}<span>DOI: {m.doi}</span>{/if}
                    {#if m.arxiv_id}<span>arXiv: {m.arxiv_id}</span>{/if}
                    {#if m.mr_number}<span>MR{m.mr_number}</span>{/if}
                    {#if m.isbn}<span>ISBN: {m.isbn}</span>{/if}
                  </div>
                </div>

                <div class="member-actions">
                  <button
                    type="button"
                    disabled={!m.is_available}
                    onclick={() => onOpenMember(m)}
                  >
                    Apri
                  </button>
                  <button
                    type="button"
                    disabled={!m.is_available}
                    onclick={() => onRevealFile(m.file_id)}
                    title="Mostra nella cartella"
                  >
                    Cartella
                  </button>
                  {#if g.status === 'pending'}
                    <button
                      type="button"
                      class="primary"
                      onclick={() => onMerge(g.group_id, m.document_id)}
                      title="Mantieni questo record bibliografico ed elimina gli altri record duplicati dal catalogo"
                    >
                      Mantieni questo
                    </button>
                  {/if}
                </div>
              </div>
            {/each}
          </div>

          <div class="group-footer">
            <button type="button" onclick={() => onOpenSideBySide(g.members)}>
              Confronta affiancati
            </button>
            {#if g.status === 'pending'}
              <button type="button" onclick={() => onResolve(g.group_id, 'distinct')}>
                Distinti
              </button>
              <button type="button" onclick={() => onResolve(g.group_id, 'versions')}>
                Versioni
              </button>
              <button type="button" onclick={() => onResolve(g.group_id, 'editions')}>
                Edizioni libro
              </button>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .dup-view {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }
  .dup-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 10px;
    border-bottom: 1px solid var(--border);
    gap: 8px;
  }
  .dup-controls {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }
  .toggle-resolved {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text-muted);
  }
  .groups-list {
    flex: 1;
    overflow-y: auto;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    contain: content;
  }
  .group-card {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px;
    background: var(--bg-panel);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .group-head {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }
  .level-badge {
    font-size: 10px;
    font-weight: 700;
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--accent-soft);
  }
  .sim {
    margin-left: auto;
    font-size: 11px;
    color: var(--text-muted);
  }
  .status-pill {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--bg-muted);
  }
  .reason {
    font-size: 11px;
    color: var(--text-muted);
  }
  .members {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .member-row {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px;
    border-radius: 4px;
    background: var(--bg-muted);
  }
  .member-row.unavailable {
    opacity: 0.6;
  }
  .member-title {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    font-weight: 600;
  }
  .fmt {
    font-size: 10px;
    padding: 1px 4px;
    border: 1px solid var(--border);
    border-radius: 3px;
  }
  .member-meta,
  .member-ids {
    font-size: 11px;
    color: var(--text-muted);
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .member-actions,
  .group-footer {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
  }
  .group-footer {
    padding-top: 4px;
    border-top: 1px solid var(--border);
  }
  .empty {
    padding: 20px;
    color: var(--text-muted);
    text-align: center;
    font-size: 13px;
  }
</style>
