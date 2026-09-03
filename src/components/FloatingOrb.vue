<script setup lang="ts">
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { profileInitials } from "../appearance";

const props = defineProps<{
  avatarUrl: string;
  avatarAvailable: boolean;
  displayName: string;
  locked: boolean;
  nativeBridgeAvailable: boolean;
}>();
const emit = defineEmits<{ expand: []; error: [message: string] }>();
const start = ref<{ x: number; y: number } | null>(null);
const dragStarted = ref(false);
const initials = computed(() => profileInitials(props.displayName));
const DRAG_THRESHOLD_DIP = 5;

function pointerDown(event: PointerEvent) {
  if (event.button !== 0) return;
  start.value = { x: event.clientX, y: event.clientY };
  dragStarted.value = false;
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
}

async function pointerMove(event: PointerEvent) {
  if (!start.value || dragStarted.value || props.locked) return;
  const distance = Math.hypot(event.clientX - start.value.x, event.clientY - start.value.y);
  if (distance <= DRAG_THRESHOLD_DIP) return;
  dragStarted.value = true;
  if (!props.nativeBridgeAvailable) return;
  try {
    await invoke("request_window_drag");
  } catch (reason) {
    emit("error", String(reason));
  }
}

function pointerUp() {
  const shouldExpand = Boolean(start.value) && !dragStarted.value;
  start.value = null;
  if (shouldExpand) emit("expand");
}

function pointerCancel() {
  start.value = null;
  dragStarted.value = false;
}
</script>

<template>
  <button
    type="button"
    class="floating-orb"
    aria-label="Open Alan Desktop"
    title="Open Alan Desktop"
    @pointerdown="pointerDown"
    @pointermove="pointerMove"
    @pointerup="pointerUp"
    @pointercancel="pointerCancel"
  >
    <img v-if="avatarAvailable" :src="avatarUrl" alt="" draggable="false" />
    <span v-else>{{ initials }}</span>
  </button>
</template>
