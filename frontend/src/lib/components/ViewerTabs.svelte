<script lang="ts">
  import type { LlmProvider, ViewerTab } from '../types';
  import UnifiedViewer from './UnifiedViewer.svelte';

  interface Props {
    tabs: ViewerTab[];
    activeTabId: string | null;
    llmProvider?: LlmProvider;
    onSelectTab: (tabId: string) => void;
    onCloseTab: (tabId: string) => void;
    onSelectMatchIndex: (tabId: string, idx: number) => void;
    onOpenExternal: (fileId: number, pageOrd: number) => void;
    onConsentAccepted?: (provider: LlmProvider) => void;
    onStatusMessage?: (msg: string) => void;
  }

  let {
    tabs,
    activeTabId,
    llmProvider = 'disabled',
    onSelectTab,
    onCloseTab,
    onSelectMatchIndex,
    onOpenExternal,
    onConsentAccepted,
    onStatusMessage,
  }: Props = $props();

  const activeTab = $derived(tabs.find((t) => t.tabId === activeTabId) ?? null);
</script>

<div class="tabs-wrapper">
  {#if tabs.length === 0}
    <div class="empty-viewer">
      Seleziona un documento o un passaggio dai risultati di ricerca (doppio clic) per aprirlo qui.
    </div>
  {:else}
    <div class="tab-bar">
      {#each tabs as tab (tab.tabId)}
        <div class="tab" class:active={tab.tabId === activeTabId}>
          <button type="button" class="tab-label" onclick={() => onSelectTab(tab.tabId)}>
            {tab.title}
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

    {#if activeTab}
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
            {onStatusMessage}
          />
        {/key}
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
    gap: 2px;
    padding: 4px 8px 0;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    overflow-x: auto;
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
  .tab-label {
    border: none;
    background: transparent;
    padding: 5px 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tab-close {
    border: none;
    background: transparent;
    padding: 4px 6px;
    font-size: 14px;
    color: var(--text-muted);
  }
  .viewer-pane {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
</style>
