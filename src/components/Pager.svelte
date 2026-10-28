<script lang="ts">
  // Kleine Seitenleiste: zurück, „Seite n von m“, weiter. Seiten zählen ab 0.
  import { t } from "../lib/i18n";

  let { page, pages, onpage }: { page: number; pages: number; onpage: (n: number) => void } = $props();
</script>

<div class="pager">
  <button class="btn small quiet" disabled={page <= 0} onclick={() => onpage(page - 1)}>‹ {t("Zurück")}</button>
  <span class="muted small">{t("Seite {n} von {total}", { n: page + 1, total: pages })}</span>
  <button class="btn small quiet" disabled={page >= pages - 1} onclick={() => onpage(page + 1)}>{t("Weiter")} ›</button>
</div>

<style>
  .pager {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    margin: 10px 0 4px;
  }
  .small {
    font-size: var(--step--1);
  }
  .btn.small {
    padding: 2px 8px;
  }
</style>
