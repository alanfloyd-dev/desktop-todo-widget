<script setup lang="ts">
import { useI18n } from "../i18n";
import type { ProductSettings } from "../types";

defineProps<{ settings: ProductSettings; x: number; y: number }>();
const emit = defineEmits<{ select: [action: string] }>();
const { t } = useI18n();

function run(action: string) {
  emit("select", action);
}
</script>

<template>
  <nav
    class="context-menu"
    :style="{ left: `${x}px`, top: `${y}px` }"
    :aria-label="t('menu.ariaLabel')"
    @pointerdown.stop
  >
    <p class="menu-heading">{{ t("menu.windowMode") }}</p>
    <button type="button" @click="run('mode.sidebar')">
      <span>{{ settings.mode === "sidebar" ? "✓" : "" }}</span>{{ t("menu.mode.sidebar") }}
    </button>
    <button type="button" @click="run('mode.floating')">
      <span>{{ settings.mode === "floating" ? "✓" : "" }}</span>{{ t("menu.mode.floating") }}
    </button>
    <button type="button" @click="run('mode.desktop')">
      <span>{{ settings.mode === "desktop" ? "✓" : "" }}</span>
      <span>{{ t("menu.mode.desktop") }} <small>{{ t("menu.desktopExperimental") }}</small></span>
    </button>

    <template v-if="settings.mode === 'sidebar'">
      <div class="menu-divider"></div>
      <p class="menu-heading">{{ t("menu.side") }}</p>
      <button type="button" @click="run('side.left')">
        <span>{{ settings.sidebarSide === "left" ? "✓" : "" }}</span>{{ t("menu.side.left") }}
      </button>
      <button type="button" @click="run('side.right')">
        <span>{{ settings.sidebarSide === "right" ? "✓" : "" }}</span>{{ t("menu.side.right") }}
      </button>
    </template>

    <div class="menu-divider"></div>
    <button type="button" @click="run('lock.toggle')">
      <span>{{ settings.locked ? "✓" : "" }}</span>{{ t("menu.lockPosition") }}
    </button>
    <button
      type="button"
      :disabled="settings.mode === 'desktop'"
      @click="run('always_on_top.toggle')"
    >
      <span>{{ settings.alwaysOnTop && settings.mode !== "desktop" ? "✓" : "" }}</span>
      {{ t("menu.alwaysOnTop") }}
    </button>
    <button
      v-if="settings.mode === 'floating'"
      type="button"
      @click="run(settings.floatingPresentation === 'collapsed' ? 'floating.expand' : 'floating.collapse')"
    >
      <span></span>
      {{
        settings.floatingPresentation === "collapsed"
          ? t("menu.expandFloating")
          : t("menu.collapseFloating")
      }}
    </button>
    <div class="menu-divider"></div>
    <button type="button" @click="run('settings')"><span></span>{{ t("menu.settings") }}</button>
    <button type="button" @click="run('quit')"><span></span>{{ t("menu.quit") }}</button>
  </nav>
</template>
