<script lang="ts">
  import type { LlmProvider, ViewerTab } from '../types';
  import UnifiedViewer from './UnifiedViewer.svelte';

  interface Props {
    tabs: ViewerTab[];
    activeTabId: string | null;
    splitTabId?: string | null;
    zenMode?: boolean;
    llmProvider?: LlmProvider;
    onSelectTab: (tabId: string) => void;
    onCloseTab: (tabId: string) => void;
    onSetSplitTab?: (tabId: string | null) => void;
    onToggleZen?: () => void;
    onSelectMatchIndex: (tabId: string, idx: number) => void;
    onOpenExternal: (fileId: number, pageOrd: number) => void;
    onConsentAccepted?: (provider: LlmProvider) => void;
    onAnnotationsChanged?: () => void;
    onStatusMessage?: (msg: string) => void;
  }

  let {
    tabs,
    activeTabId,
    splitTabId = null,
    zenMode = false,
    llmProvider = 'disabled',
    onSelectTab,
    onCloseTab,
    onSetSplitTab,
    onToggleZen,
    onSelectMatchIndex,
    onOpenExternal,
    onConsentAccepted,
    onAnnotationsChanged,
    onStatusMessage,
  }: Props = $props();

  const activeTab = $derived(tabs.find((t) => t.tabId === activeTabId) ?? null);
  const effectiveSplitTab = $derived(
    splitTabId && splitTabId !== activeTabId
      ? (tabs.find((t) => t.tabId === splitTabId) ?? null)
      : null
  );

  function toggleSplitMode() {
    if (!onSetSplitTab) return;
    if (effectiveSplitTab) {
      onSetSplitTab(null);
      return;
    }
    const other = tabs.find((t) => t.tabId !== activeTabId);
    if (other) {
      onSetSplitTab(other.tabId);
    }
  }
</script>

<div class="tabs-wrapper">
  {#if tabs.length === 0}
    <div class="empty-viewer">
      Seleziona un documento o un passaggio dai risultati di ricerca (doppio clic) per aprirlo qui.
    </div>
  {:else}
    <div class="tab-bar">
      <div class="tab-list">
        {#each tabs as tab (tab.tabId)}
          {@const isPrimary = tab.tabId === activeTabId}
          {@const isSecondary = effectiveSplitTab?.tabId === tab.tabId}
          <div class="tab" class:active={isPrimary} class:split-active={isSecondary}>
            <button
              type="button"
              class="tab-label"
              onclick={() => {
                if (isSecondary && onSetSplitTab && activeTabId) {
                  // Swap primary & split cleanly
                  const prev = activeTabId;
                  onSelectTab(tab.tabId);
                  onSetSplitTab(prev);
                } else {
                  onSelectTab(tab.tabId);
                }
              }}
              oncontextmenu={(e) => {
                if (tabs.length >= 2 && onSetSplitTab && tab.tabId !== activeTabId) {
                  e.preventDefault();
                  onSetSplitTab(isSecondary ? null : tab.tabId);
                }
              }}
              title={tabs.length >= 2 && tab.tabId !== activeTabId
                ? `${tab.title} (tasto destro per affiancare a destra)`
                : tab.title}
            >
              {#if isSecondary}
                <span class="split-badge" title="Affiancato a destra">◫</span>
              {/if}
              <span>{tab.title}</span>
            </button>
            <button
              type="button"
              class="tab-close"
              title="Chiudi scheda"
              onclick={() => onCloseTab(tab.tabId)}
            >
              ×
            </button>
          </div>
        {/each}
      </div>

      <div class="tab-bar-tools">
        {#if tabs.length >= 2 && onSetSplitTab}
          <button
            type="button"
            class="tb-tool-btn"
            class:active={Boolean(effectiveSplitTab)}
            title={effectiveSplitTab
              ? 'Chiudi vista affiancata di 2 documenti'
              : 'Affianca 2 documenti aperti (vista doppia)'}
            aria-label="Affianca due documenti"
            onclick={toggleSplitMode}
          >
            <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <rect x="3" y="4" width="18" height="16" rx="2" />
              <path d="M12 4v16" />
            </svg>
          </button>
        {/if}
        {#if onToggleZen}
          <button
            type="button"
            class="tb-tool-btn"
            class:active={zenMode}
            title="Modalità lettura Zen (⌘\)"
            aria-label="Modalità lettura Zen"
            onclick={onToggleZen}
          >
            <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              {#if zenMode}
                <path d="M8 3v3a2 2 0 0 1-2 2H3m18 0h-3a2 2 0 0 1-2-2V3m0 18v-3a2 2 0 0 1 2-2h3M3 16h3a2 2 0 0 1 2 2v3" />
              {:else}
                <path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7" />
              {/if}
            </svg>
          </button>
        {/if}
      </div>
    </div>

    {#if activeTab}
      <div class="viewer-panes" class:split={Boolean(effectiveSplitTab)}>
        <div class="viewer-pane">
          {#key activeTab.tabId}
            <UnifiedViewer
              fileId={activeTab.fileId}
              targetPage={activeTab.targetPage}
              explicitPageJump={Boolean(activeTab.explicitPageJump)}
              highlightRects={activeTab.highlightRects}
              matches={activeTab.matches}
              activeMatchIndex={activeTab.activeMatchIndex}
              {llmProvider}
              onSelectMatchIndex={(idx) => onSelectMatchIndex(activeTab.tabId, idx)}
              {onOpenExternal}
              {onConsentAccepted}
              {onAnnotationsChanged}
              {onStatusMessage}
            />
          {/key}
        </div>

        {#if effectiveSplitTab}
          <div class="viewer-pane split-right">
            <div class="split-pane-head">
              <span class="split-pane-title" title={effectiveSplitTab.title}>
                {effectiveSplitTab.title}
              </span>
              <button
                type="button"
                class="split-pane-close"
                title="Chiudi pannello affiancato"
                onclick={() => onSetSplitTab?.(null)}
              >
                ×
              </button>
            </div>
            <div class="split-pane-body">
              {#key effectiveSplitTab.tabId}
                <UnifiedViewer
                  fileId={effectiveSplitTab.fileId}
                  targetPage={effectiveSplitTab.targetPage}
                  explicitPageJump={Boolean(effectiveSplitTab.explicitPageJump)}
                  highlightRects={effectiveSplitTab.highlightRects}
                  matches={effectiveSplitTab.matches}
                  activeMatchIndex={effectiveSplitTab.activeMatchIndex}
                  {llmProvider}
                  onSelectMatchIndex={(idx) => onSelectMatchIndex(effectiveSplitTab.tabId, idx)}
                  {onOpenExternal}
                  {onConsentAccepted}
                  {onAnnotationsChanged}
                  {onStatusMessage}
                />
              {/key}
            </div>
          </div>
        {/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .tabs-wrapper {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }
  .empty-viewer {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
    padding: 24px;
    text-align: center;
  }
  .tab-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    padding: 4px 8px 0;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
  }
  .tab-list {
    display: flex;
    gap: 2px;
    overflow-x: auto;
    min-width: 0;
    flex: 1;
  }
  .tab-bar-tools {
    display: flex;
    align-items: center;
    gap: 3px;
    padding-bottom: 3px;
    flex-shrink: 0;
  }
  .tb-tool-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border-radius: 5px;
    border: 1px solid transparent;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }
  .tb-tool-btn:hover {
    background: var(--bg-muted);
    color: var(--text);
  }
  .tb-tool-btn.active {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
  }
  .tab {
    display: flex;
    align-items: center;
    border: 1px solid transparent;
    border-bottom: none;
    border-radius: 6px 6px 0 0;
    background: var(--bg-muted);
    max-width: 240px;
  }
  .tab.active {
    background: var(--bg);
    border-color: var(--border);
  }
  .tab.split-active {
    background: var(--bg);
    border-color: var(--accent);
    opacity: 0.92;
  }
  .tab-label {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border: none;
    background: transparent;
    padding: 5px 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .split-badge {
    font-size: 11px;
    color: var(--accent);
    flex-shrink: 0;
  }
  .tab-close {
    border: none;
    background: transparent;
    padding: 4px 6px;
    font-size: 14px;
    color: var(--text-muted);
  }
  .viewer-panes {
    flex: 1;
    min-height: 0;
    display: flex;
    overflow: hidden;
  }
  .viewer-pane {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
  .viewer-pane.split-right {
    border-left: 2px solid var(--border);
  }
  .split-pane-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    padding: 3px 8px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    font-size: 11px;
    color: var(--text-muted);
  }
  .split-pane-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
    color: var(--text);
  }
  .split-pane-close {
    border: none;
    background: transparent;
    color: var(--text-muted);
    padding: 0 4px;
    font-size: 14px;
    cursor: pointer;
  }
  .split-pane-body {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
</style>
