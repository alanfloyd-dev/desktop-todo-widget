<script setup lang="ts">
import { computed } from "vue";
import type { AppearanceSettings, BackgroundType, ResolvedContrast } from "../types";

const props = defineProps<{
  appearance: AppearanceSettings;
  imageUrl: string;
  imageAvailable: boolean;
  wallpaperUrl: string;
  wallpaperAvailable: boolean;
  resolvedContrast: ResolvedContrast;
}>();

const effectiveType = computed<BackgroundType>(() => {
  if (props.appearance.backgroundType === "image" && !props.imageAvailable) return "glass";
  if (props.appearance.backgroundType === "wallpaper" && !props.wallpaperAvailable) return "glass";
  return props.appearance.backgroundType;
});

const sourceUrl = computed(() => {
  if (effectiveType.value === "image") return props.imageUrl;
  if (effectiveType.value === "wallpaper") return props.wallpaperUrl;
  return "";
});

const materialStyle = computed(() => {
  const appearance = props.appearance;
  const fit = appearance.imageFit === "stretch" ? "100% 100%" : appearance.imageFit;
  const position = appearance.imagePosition;
  const background = effectiveType.value === "glass"
    ? "transparent"
    : effectiveType.value === "solid"
    ? appearance.solidColor
    : effectiveType.value === "gradient"
      ? `linear-gradient(${appearance.gradientAngle}deg, ${appearance.gradientStartColor}, ${appearance.gradientEndColor})`
      : sourceUrl.value
        ? `url("${sourceUrl.value}")`
        : "transparent";
  return {
    "--material-background": background,
    "--material-size": sourceUrl.value ? fit : "cover",
    "--material-position": position,
    "--material-opacity": String(effectiveType.value === "solid" ? 1 : appearance.backgroundOpacity),
    "--material-blur": `${effectiveType.value === "glass" ? appearance.blurPx : 0}px`,
    "--glass-tint": appearance.glassTintColor,
    "--glass-tint-opacity": String(effectiveType.value === "glass" ? appearance.glassTintOpacity : 0),
    "--overlay-color": props.resolvedContrast === "light" ? "0, 0, 0" : "255, 255, 255",
    "--overlay-opacity": String(appearance.overlayStrength),
  };
});
</script>

<template>
  <div class="appearance-material" :data-background-type="effectiveType" :style="materialStyle" aria-hidden="true">
    <div class="appearance-background-layer"></div>
    <div class="appearance-backdrop-layer"></div>
    <div class="appearance-tint-layer"></div>
    <div class="appearance-overlay-layer"></div>
  </div>
</template>
