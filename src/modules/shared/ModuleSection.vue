<script setup lang="ts">
import { computed, useId } from "vue";
import { useI18n } from "../../i18n";

/**
 * The shared collapsible module container (v1.3.0-D1).
 *
 * One grid-neutral section: the header is always visible and carries the
 * module title plus the collapse toggle; the body is the `v-show` wrapper the
 * module's content renders into. Collapsing hides only the body — nothing
 * inside is unmounted, so a module's ephemeral state (Todo's draft, its drag
 * listeners) survives untouched, and a collapsed module keeps its grid
 * placement while shrinking to the header's natural height.
 *
 * Deliberately absent: module icons, animation, a window drag region, and an
 * internal scrolling container. Weather is not wrapped in this container.
 */
const props = withDefaults(
  defineProps<{
    /** The module's visible heading text, shown in every collapse state. */
    title: string;
    /**
     * The stored collapse state; `true` renders the body hidden with an
     * expand chevron. Defaults to expanded.
     */
    collapsed?: boolean;
    /**
     * The module's name for the toggle's accessible label ("Collapse Todo"),
     * which should read as a name rather than the possibly styled heading
     * text. Defaults to the title.
     */
    moduleName?: string;
  }>(),
  { collapsed: false, moduleName: undefined },
);

const emit = defineEmits<{ toggle: [] }>();

const { t } = useI18n();

const toggleLabel = computed(() =>
  t(props.collapsed ? "modules.expandAction" : "modules.collapseAction", {
    module: props.moduleName ?? props.title,
  }),
);

const bodyId = useId();
</script>

<!--
  A single DOM root: class and aria-label fall through from the caller onto
  this section, so Quick Links keeps its `.quick-links` identity and landmark
  name without a wrapper.
-->
<template>
  <section class="module-section" :class="{ 'module-section--collapsed': collapsed }">
    <div class="section-heading module-section-heading">
      <!--
        The title slot exists so a module whose heading is a real heading
        element (Todo's `h1`) keeps exactly one heading — the header replaces
        the module's own heading row rather than adding a second one.
      -->
      <slot name="title">
        <span class="module-section-title">{{ title }}</span>
      </slot>
      <div class="section-heading-actions">
        <slot name="actions" />
        <button
          type="button"
          class="module-section-toggle"
          :aria-expanded="!collapsed"
          :aria-controls="bodyId"
          :aria-label="toggleLabel"
          :title="toggleLabel"
          @click="emit('toggle')"
        >
          <svg
            class="module-section-chevron"
            viewBox="0 0 24 24"
            aria-hidden="true"
            focusable="false"
          >
            <path
              d="M7 9.6 12 14.4 17 9.6"
              fill="none"
              stroke="currentColor"
              stroke-width="1.7"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
        </button>
      </div>
    </div>
    <!--
      Visibility-only collapse: `v-show`, not `v-if`. The module keeps its
      DOM, its draft input state, and its window-level listeners while
      collapsed, and contributes no height.
    -->
    <div :id="bodyId" v-show="!collapsed" class="module-section-body">
      <slot />
    </div>
  </section>
</template>
