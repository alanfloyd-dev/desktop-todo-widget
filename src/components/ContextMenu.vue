<script setup lang="ts">
import type { ProductSettings } from "../types";

defineProps<{ settings: ProductSettings; x: number; y: number }>();
const emit = defineEmits<{ select: [action: string] }>();

function run(action: string) {
  emit("select", action);
}
</script>

<template>
  <nav
    class="context-menu"
    :style="{ left: `${x}px`, top: `${y}px` }"
    aria-label="Window menu"
    @pointerdown.stop
  >
    <p class="menu-heading">Window mode</p>
    <button type="button" @click="run('mode.sidebar')">
      <span>{{ settings.mode === "sidebar" ? "✓" : "" }}</span>Sidebar
    </button>
    <button type="button" @click="run('mode.floating')">
      <span>{{ settings.mode === "floating" ? "✓" : "" }}</span>Floating
    </button>
    <button type="button" @click="run('mode.desktop')">
      <span>{{ settings.mode === "desktop" ? "✓" : "" }}</span>
      <span>Desktop <small>Experimental</small></span>
    </button>

    <template v-if="settings.mode === 'sidebar'">
      <div class="menu-divider"></div>
      <p class="menu-heading">Side</p>
      <button type="button" @click="run('side.left')">
        <span>{{ settings.sidebarSide === "left" ? "✓" : "" }}</span>Left
      </button>
      <button type="button" @click="run('side.right')">
        <span>{{ settings.sidebarSide === "right" ? "✓" : "" }}</span>Right
      </button>
    </template>

    <div class="menu-divider"></div>
    <button type="button" @click="run('lock.toggle')">
      <span>{{ settings.locked ? "✓" : "" }}</span>Lock position
    </button>
    <button
      type="button"
      :disabled="settings.mode === 'desktop'"
      @click="run('always_on_top.toggle')"
    >
      <span>{{ settings.alwaysOnTop && settings.mode !== "desktop" ? "✓" : "" }}</span>
      Always on top
    </button>
    <button
      v-if="settings.mode === 'floating'"
      type="button"
      @click="run(settings.floatingPresentation === 'collapsed' ? 'floating.expand' : 'floating.collapse')"
    >
      <span></span>
      {{ settings.floatingPresentation === "collapsed" ? "Expand Floating" : "Collapse to Avatar Orb" }}
    </button>
    <div class="menu-divider"></div>
    <button type="button" @click="run('settings')"><span></span>Settings</button>
    <button type="button" @click="run('quit')"><span></span>Quit</button>
  </nav>
</template>
