<script lang="ts">
  import { tr, type Lang } from '../i18n';
  import type {
    MscTreeNode,
    SourceRoot,
    TagSummary,
    TopicClusterView,
  } from '../types';

  interface Props {
    lang?: Lang;
    collapsed: boolean;
    onToggleCollapse: () => void;
    mscTree: MscTreeNode[];
    clusters: TopicClusterView[];
    tags: TagSummary[];
    roots: SourceRoot[];
    activeMscPrefix: string | null;
    activeClusterId: number | null;
    activeTag: string | null;
    activeRootId: string | null;
    knnBusy: boolean;
    clusteringBusy: boolean;
    onSelectMsc: (code: string | null) => void;
    onSelectCluster: (clusterId: number | null) => void;
    onSelectTag: (tagName: string | null) => void;
    onSelectRoot: (rootId: string | null) => void;
    onClearAllFilters: () => void;
    onRunKnnProposals: () => Promise<void>;
    onRunClustering: () => Promise<void>;
    onRenameCluster: (clusterId: number, label: string) => Promise<void>;
  }

  let {
    lang = 'it',
    collapsed,
    onToggleCollapse,
    mscTree,
    clusters,
    tags,
    roots,
    activeMscPrefix,
    activeClusterId,
    activeTag,
    activeRootId,
    knnBusy,
    clusteringBusy,
    onSelectMsc,
    onSelectCluster,
    onSelectTag,
    onSelectRoot,
    onClearAllFilters,
    onRunKnnProposals,
    onRunClustering,
    onRenameCluster,
  }: Props = $props();

  let expandedMsc = $state<Record<string, boolean>>({});
  let showAllMsc = $state(false);
  let editingClusterId = $state<number | null>(null);
  let editingClusterLabel = $state('');

  let hasAnyFilter = $derived(
    activeMscPrefix !== null ||
      activeClusterId !== null ||
      activeTag !== null ||
      activeRootId !== null
  );

  let visibleMscNodes = $derived(
    showAllMsc ? mscTree : mscTree.filter((n) => n.total_count > 0 || n.code === activeMscPrefix)
  );

  function toggleMscExpand(code: string) {
    expandedMsc = { ...expandedMsc, [code]: !expandedMsc[code] };
  }

  function startRename(cluster: TopicClusterView) {
    editingClusterId = cluster.id;
    editingClusterLabel = cluster.label;
  }

  async function commitRename(clusterId: number) {
    const trimmed = editingClusterLabel.trim();
    if (!trimmed) {
      editingClusterId = null;
      return;
    }
    await onRenameCluster(clusterId, trimmed);
    editingClusterId = null;
  }
</script>

{#if collapsed}
  <aside class="sidebar-rail" aria-label="Barra laterale navigazione chiusa">
    <button
      class="rail-btn"
      onclick={onToggleCollapse}
      title="Espandi barra classificazioni MSC, gruppi e tag"
    >
      ›
    </button>
    {#if hasAnyFilter}
      <button class="rail-clear" onclick={onClearAllFilters} title="Rimuovi filtri attivi">
        ✕
      </button>
    {/if}
  </aside>
{:else}
  <aside class="sidebar" aria-label={tr(lang, 'taxonomy')}>
    <div class="sb-header">
      <span class="sb-title">{tr(lang, 'taxonomy')}</span>
      <div class="sb-header-actions">
        {#if hasAnyFilter}
          <button class="mini-btn clear" onclick={onClearAllFilters} title={tr(lang, 'btnClear')}>
            {tr(lang, 'btnClear')}
          </button>
        {/if}
        <button class="icon-btn" onclick={onToggleCollapse} title="Comprimi barra">‹</button>
      </div>
    </div>

    <div class="sb-scroll">
      <!-- MSC2020 -->
      <section class="sb-section">
        <div class="sec-head">
          <span class="sec-title" title={tr(lang, 'mscSection')}>{tr(lang, 'mscSection')}</span>
          <div class="sec-actions">
            <button
              class="sb-icon-btn"
              class:active={showAllMsc}
              onclick={() => (showAllMsc = !showAllMsc)}
              title={showAllMsc
                ? 'Mostra solo classi MSC popolate'
                : 'Mostra tutte le 63 sezioni MSC2020'}
              aria-label={showAllMsc ? tr(lang, 'showPopulatedMsc') : tr(lang, 'showAllMsc')}
            >
              <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <path d="M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6-10-6-10-6z" />
                <circle cx="12" cy="12" r="2.5" />
              </svg>
            </button>
            <button
              class="sb-icon-btn accent"
              disabled={knnBusy}
              onclick={() => onRunKnnProposals()}
              title="Proponi codici MSC tramite k-NN sugli embeddings locali"
              aria-label={tr(lang, 'proposeKnn')}
            >
              <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <path d="m12 3 1.8 4.2L18 9l-4.2 1.8L12 15l-1.8-4.2L6 9l4.2-1.8z" />
                <path d="m19 15 .9 2.1L22 18l-2.1.9L19 21l-.9-2.1L16 18l2.1-.9z" />
              </svg>
            </button>
          </div>
        </div>

        {#if visibleMscNodes.length === 0}
          <p class="empty-note">Nessuna classe MSC assegnata. Clicca “Tutte” o modifica i metadati.</p>
        {:else}
          <ul class="tree-list">
            {#each visibleMscNodes as node (node.code)}
              {@const isSelected = activeMscPrefix === node.code}
              {@const hasKids = node.children.length > 0}
              {@const isOpen = !!expandedMsc[node.code]}
              <li>
                <div class="tree-row" class:selected={isSelected}>
                  {#if hasKids}
                    <button
                      class="twist-btn"
                      onclick={() => toggleMscExpand(node.code)}
                      aria-label={isOpen ? 'Comprimi sottoclassi' : 'Espandi sottoclassi'}
                    >
                      {isOpen ? '▾' : '▸'}
                    </button>
                  {:else}
                    <span class="twist-spacer"></span>
                  {/if}
                  <button
                    class="row-label-btn"
                    onclick={() => onSelectMsc(isSelected ? null : node.code)}
                    title="{node.code} — {node.title}"
                  >
                    <span class="msc-code">{node.code}</span>
                    <span class="row-text">{node.title}</span>
                  </button>
                  <span class="counts">
                    {#if node.confirmed_count > 0}
                      <span class="cnt confirmed" title="Confermati">{node.confirmed_count}</span>
                    {/if}
                    {#if node.proposed_count > 0}
                      <span class="cnt proposed" title="Proposti da confermare">+{node.proposed_count}</span>
                    {/if}
                  </span>
                </div>

                {#if hasKids && isOpen}
                  <ul class="tree-sublist">
                    {#each node.children as child (child.code)}
                      {@const childSelected = activeMscPrefix === child.code}
                      <li>
                        <div class="tree-row child" class:selected={childSelected}>
                          <button
                            class="row-label-btn"
                            onclick={() => onSelectMsc(childSelected ? null : child.code)}
                            title={child.code}
                          >
                            <span class="msc-code leaf">{child.code}</span>
                          </button>
                          <span class="counts">
                            {#if child.confirmed_count > 0}
                              <span class="cnt confirmed">{child.confirmed_count}</span>
                            {/if}
                            {#if child.proposed_count > 0}
                              <span class="cnt proposed">+{child.proposed_count}</span>
                            {/if}
                          </span>
                        </div>
                      </li>
                    {/each}
                  </ul>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      <!-- Gruppi Tematici (Clustering) -->
      <section class="sb-section">
        <div class="sec-head">
          <span class="sec-title" title={tr(lang, 'clustersSection')}>{tr(lang, 'clustersSection')}</span>
          <button
            class="sb-icon-btn accent"
            disabled={clusteringBusy}
            onclick={() => onRunClustering()}
            title="Calcola cluster tematici (k-means sferico + c-TF-IDF) sugli embeddings locali"
            aria-label={tr(lang, 'clusterize')}
          >
            <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <circle cx="12" cy="6" r="3" />
              <circle cx="6" cy="18" r="3" />
              <circle cx="18" cy="18" r="3" />
              <path d="M10.5 8.5 7.5 15.5M13.5 8.5l3 7" />
            </svg>
          </button>
        </div>

        {#if clusters.length === 0}
          <p class="empty-note">Nessun gruppo generato. Indicizza gli embeddings e premi Clusterizza.</p>
        {:else}
          <ul class="cluster-list">
            {#each clusters as cl (cl.id)}
              {@const clSelected = activeClusterId === cl.id}
              <li class="cluster-item" class:selected={clSelected}>
                {#if editingClusterId === cl.id}
                  <form
                    class="rename-row"
                    onsubmit={(e) => {
                      e.preventDefault();
                      commitRename(cl.id);
                    }}
                  >
                    <input
                      type="text"
                      bind:value={editingClusterLabel}
                      onkeydown={(e) => {
                        if (e.key === 'Escape') editingClusterId = null;
                      }}
                    />
                    <button type="submit" class="mini-btn accent">OK</button>
                  </form>
                {:else}
                  <div class="cluster-top">
                    <button
                      class="cluster-btn"
                      onclick={() => onSelectCluster(clSelected ? null : cl.id)}
                      title={cl.keywords.join(', ')}
                    >
                      <span class="cl-label">
                        {#if cl.is_label_locked}🔒 {/if}{cl.label}
                      </span>
                      <span class="cnt confirmed">{cl.document_count}</span>
                    </button>
                    <button
                      class="edit-btn"
                      onclick={() => startRename(cl)}
                      title="Rinomina e blocca etichetta"
                    >
                      ✎
                    </button>
                  </div>
                  {#if cl.keywords.length > 0}
                    <div class="kw-row">{cl.keywords.slice(0, 4).join(' · ')}</div>
                  {/if}
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      <!-- Tag liberi -->
      <section class="sb-section">
        <div class="sec-head">
          <span class="sec-title">{tr(lang, 'tagsSection')}</span>
        </div>
        {#if tags.length === 0}
          <p class="empty-note">Nessun tag assegnato.</p>
        {:else}
          <div class="tag-cloud">
            {#each tags as tg (tg.id)}
              {@const tgSelected = activeTag === tg.name}
              <button
                class="tag-pill"
                class:selected={tgSelected}
                onclick={() => onSelectTag(tgSelected ? null : tg.name)}
              >
                <span>#{tg.name}</span>
                <span class="tag-cnt">{tg.document_count}</span>
              </button>
            {/each}
          </div>
        {/if}
      </section>

      <!-- Cartelle sorgente -->
      <section class="sb-section">
        <div class="sec-head">
          <span class="sec-title">{tr(lang, 'foldersSection')}</span>
        </div>
        <ul class="tree-list">
          {#each roots as root (root.id)}
            {@const rtSelected = activeRootId === root.id}
            <li>
              <div class="tree-row" class:selected={rtSelected}>
                <span class="dot" class:online={root.is_reachable}></span>
                <button
                  class="row-label-btn"
                  onclick={() => onSelectRoot(rtSelected ? null : root.id)}
                  title={root.is_reachable
                    ? root.root_path
                    : `${root.root_path} — ${tr(lang, 'rootUnreachableBadge')}`}
                >
                  <span class="row-text">{root.label}</span>
                </button>
              </div>
            </li>
          {/each}
        </ul>
      </section>
    </div>
  </aside>
{/if}

<style>
  .sidebar-rail {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 8px 3px;
    background: var(--bg-muted);
    border-right: 1px solid var(--border);
    width: 28px;
  }

  .rail-btn,
  .rail-clear {
    width: 22px;
    height: 24px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--text);
    cursor: pointer;
    font-size: 13px;
    line-height: 1;
    padding: 0;
  }

  .rail-clear {
    color: var(--danger);
    border-color: var(--danger);
  }

  .sidebar {
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
    color: var(--text);
    border-right: 1px solid var(--border);
    height: 100%;
    overflow: hidden;
  }

  .sb-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 10px;
    background: var(--bg-muted);
    border-bottom: 1px solid var(--border);
  }

  .sb-title {
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-muted);
  }

  .sb-header-actions {
    display: flex;
    align-items: center;
    gap: 5px;
  }

  .sb-scroll {
    flex: 1;
    overflow-y: auto;
    padding: 6px 8px 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .sb-section {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }

  .sec-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    min-width: 0;
  }

  .sec-title {
    font-size: 11px;
    font-weight: 700;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .sec-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }

  .sb-icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--text);
    flex-shrink: 0;
  }

  .sb-icon-btn.active,
  .sb-icon-btn.accent {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
  }

  .mini-btn {
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--text);
    border-radius: 5px;
    padding: 2px 6px;
    font-size: 10.5px;
    font-weight: 600;
    cursor: pointer;
  }

  .mini-btn:hover:not(:disabled) {
    background: var(--bg-muted);
  }

  .mini-btn.accent {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
  }

  .mini-btn.clear {
    border-color: var(--danger);
    background: rgba(220, 38, 38, 0.12);
    color: var(--danger);
  }

  .mini-btn:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .icon-btn {
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--text);
    border-radius: 5px;
    width: 22px;
    height: 22px;
    cursor: pointer;
    line-height: 1;
    font-size: 13px;
  }

  .empty-note {
    margin: 2px 0;
    font-size: 11px;
    color: var(--text-muted);
    line-height: 1.35;
  }

  .tree-list,
  .tree-sublist,
  .cluster-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .tree-sublist {
    padding-left: 16px;
    margin-top: 2px;
  }

  .tree-row {
    display: flex;
    align-items: center;
    gap: 4px;
    border-radius: 6px;
    padding: 3px 5px;
    font-size: 12px;
  }

  .tree-row:hover {
    background: var(--bg-muted);
  }

  .tree-row.selected {
    background: var(--accent-soft);
  }

  .twist-btn,
  .twist-spacer {
    width: 14px;
    flex-shrink: 0;
    border: none;
    background: transparent;
    padding: 0;
    cursor: pointer;
    color: var(--text-muted);
    font-size: 11px;
  }

  .row-label-btn {
    flex: 1;
    min-width: 0;
    border: none;
    background: transparent;
    padding: 0;
    display: flex;
    align-items: baseline;
    gap: 5px;
    text-align: left;
    cursor: pointer;
    color: var(--text);
  }

  .msc-code {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    font-weight: 700;
    color: var(--accent);
    flex-shrink: 0;
  }

  .msc-code.leaf {
    color: var(--accent);
    opacity: 0.88;
  }

  .row-text {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 11.5px;
  }

  .counts {
    display: flex;
    gap: 3px;
    flex-shrink: 0;
  }

  .cnt {
    font-size: 10px;
    font-weight: 700;
    padding: 0 5px;
    border-radius: 999px;
  }

  .cnt.confirmed {
    background: var(--bg-muted);
    color: var(--text);
  }

  .cnt.proposed {
    background: rgba(217, 119, 6, 0.16);
    color: var(--warn);
    border: 1px dashed var(--warn);
  }

  .cluster-item {
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--bg-panel);
    padding: 5px 7px;
  }

  .cluster-item.selected {
    border-color: var(--accent);
    background: var(--accent-soft);
  }

  .cluster-top {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .cluster-btn {
    flex: 1;
    min-width: 0;
    border: none;
    background: transparent;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    text-align: left;
    cursor: pointer;
  }

  .cl-label {
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .edit-btn {
    border: none;
    background: transparent;
    cursor: pointer;
    font-size: 11px;
    color: var(--text-muted);
    padding: 0 3px;
  }

  .edit-btn:hover {
    color: var(--text);
  }

  .kw-row {
    font-size: 10px;
    color: var(--text-muted);
    margin-top: 2px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .rename-row {
    display: flex;
    gap: 4px;
  }

  .rename-row input {
    flex: 1;
    min-width: 0;
    border: 1px solid var(--accent);
    background: var(--bg-panel);
    color: var(--text);
    border-radius: 4px;
    padding: 2px 5px;
    font-size: 11.5px;
  }

  .tag-cloud {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .tag-pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--text);
    border-radius: 999px;
    padding: 2px 8px;
    font-size: 11px;
    cursor: pointer;
  }

  .tag-pill.selected {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
  }

  .tag-cnt {
    opacity: 0.75;
    font-size: 10px;
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-muted);
    flex-shrink: 0;
  }

  .dot.online {
    background: #16a34a;
  }
</style>
