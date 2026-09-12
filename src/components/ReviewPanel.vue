<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useI18n } from "../i18n";
import type {
  ReviewCounts,
  ReviewPeriod,
  ReviewReport,
  ReviewTask,
} from "../types";

const props = defineProps<{ nativeBridgeAvailable: boolean }>();
const emit = defineEmits<{ close: []; error: [message: string] }>();
const { t, intlLocale } = useI18n();

const period = ref<ReviewPeriod>("daily");
const anchorDate = ref<string | null>(null);
const report = ref<ReviewReport | null>(null);
const loading = ref(true);
const closeButton = ref<HTMLButtonElement>();

const periods: Array<{ value: ReviewPeriod; labelKey: string }> = [
  { value: "daily", labelKey: "review.period.daily" },
  { value: "weekly", labelKey: "review.period.weekly" },
  { value: "monthly", labelKey: "review.period.monthly" },
];
const countRows: Array<{ key: keyof ReviewCounts; labelKey: string }> = [
  { key: "planned", labelKey: "review.count.planned" },
  { key: "completed", labelKey: "review.count.completed" },
  { key: "carried", labelKey: "review.count.carried" },
  { key: "cancelled", labelKey: "review.count.cancelled" },
  { key: "pending", labelKey: "review.count.pending" },
];

const title = computed(() => t(`review.title.${period.value}`));
const activeDays = computed(() => report.value?.days.filter((day) => day.counts.planned > 0) ?? []);
const canMoveForward = computed(
  () => !!report.value && report.value.periodEnd < report.value.currentTaskDay,
);

function utcDate(value: string) {
  const [year, month, day] = value.split("-").map(Number);
  return new Date(Date.UTC(year, month - 1, day));
}

function isoDate(value: Date) {
  return value.toISOString().slice(0, 10);
}

function longDate(value: string) {
  return new Intl.DateTimeFormat(intlLocale.value, {
    day: "2-digit",
    month: "short",
    year: "numeric",
    timeZone: "UTC",
  }).format(utcDate(value));
}

function shortDate(value: string) {
  return new Intl.DateTimeFormat(intlLocale.value, {
    weekday: "short",
    day: "2-digit",
    month: "short",
    timeZone: "UTC",
  }).format(utcDate(value));
}

function periodLabel(value: ReviewReport) {
  if (value.period === "daily") return longDate(value.anchorDate);
  if (value.period === "monthly") {
    return new Intl.DateTimeFormat(intlLocale.value, {
      month: "long",
      year: "numeric",
      timeZone: "UTC",
    }).format(utcDate(value.periodStart));
  }
  return `${longDate(value.periodStart)} — ${longDate(value.periodEnd)}`;
}

function statusFacts(counts: ReviewCounts) {
  const facts: string[] = [];
  if (counts.completed) facts.push(t("review.fact.completed", { count: counts.completed }));
  if (counts.carried) facts.push(t("review.fact.carried", { count: counts.carried }));
  if (counts.cancelled) facts.push(t("review.fact.cancelled", { count: counts.cancelled }));
  if (counts.pending) facts.push(t("review.fact.pending", { count: counts.pending }));
  return facts.length ? facts.join(" · ") : t("review.noTasksPlanned");
}

function taskStatus(status: ReviewTask["status"]) {
  return t(`task.status.${status}`);
}

function taskFacts(task: ReviewTask) {
  const facts = [taskStatus(task.status)];
  if (task.completedAt !== null) {
    facts.push(new Intl.DateTimeFormat(intlLocale.value, {
      hour: "2-digit",
      minute: "2-digit",
    }).format(new Date(task.completedAt * 1000)));
  }
  if (task.categoryName) facts.push(task.categoryName);
  if (task.carriedFrom) facts.push(t("review.carriedForward"));
  return facts.join(" · ");
}

function browserTaskDay() {
  const now = new Date();
  if (now.getHours() < 4) now.setDate(now.getDate() - 1);
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
}

function shiftDate(value: string, selectedPeriod: ReviewPeriod, amount: number) {
  const date = utcDate(value);
  if (selectedPeriod === "daily") date.setUTCDate(date.getUTCDate() + amount);
  if (selectedPeriod === "weekly") date.setUTCDate(date.getUTCDate() + amount * 7);
  if (selectedPeriod === "monthly") date.setUTCMonth(date.getUTCMonth() + amount);
  return isoDate(date);
}

function bounds(selectedPeriod: ReviewPeriod, anchor: string) {
  const selected = utcDate(anchor);
  if (selectedPeriod === "daily") return { start: anchor, end: anchor };
  if (selectedPeriod === "weekly") {
    const mondayOffset = (selected.getUTCDay() + 6) % 7;
    selected.setUTCDate(selected.getUTCDate() - mondayOffset);
    const start = isoDate(selected);
    selected.setUTCDate(selected.getUTCDate() + 6);
    return { start, end: isoDate(selected) };
  }
  selected.setUTCDate(1);
  const start = isoDate(selected);
  selected.setUTCMonth(selected.getUTCMonth() + 1);
  selected.setUTCDate(0);
  return { start, end: isoDate(selected) };
}

function browserReport(selectedPeriod: ReviewPeriod, requestedAnchor: string | null): ReviewReport {
  const currentTaskDay = browserTaskDay();
  const anchor = requestedAnchor ?? currentTaskDay;
  const range = bounds(selectedPeriod, anchor);
  const tasks: ReviewTask[] = [
    { id: "review-1", title: "Prepare field notes", categoryName: "Research", status: "completed", scheduledDate: anchor, completedAt: 1788217200, carriedFrom: null },
    { id: "review-2", title: "Update specimen index", categoryName: "Research", status: "carried", scheduledDate: anchor, completedAt: null, carriedFrom: null },
    { id: "review-3", title: "Book archive visit", categoryName: "Admin", status: "cancelled", scheduledDate: anchor, completedAt: null, carriedFrom: null },
    { id: "review-4", title: "Read survey summary", categoryName: null, status: "pending", scheduledDate: anchor, completedAt: null, carriedFrom: "review-earlier" },
  ];
  const totals: ReviewCounts = { planned: 4, completed: 1, carried: 1, cancelled: 1, pending: 1 };
  const days = [];
  let cursor = utcDate(range.start);
  const finish = utcDate(range.end);
  while (cursor <= finish) {
    const date = isoDate(cursor);
    days.push({ date, counts: date === anchor ? totals : { planned: 0, completed: 0, carried: 0, cancelled: 0, pending: 0 } });
    cursor.setUTCDate(cursor.getUTCDate() + 1);
  }
  return {
    period: selectedPeriod,
    anchorDate: anchor,
    periodStart: range.start,
    periodEnd: range.end,
    currentTaskDay,
    totals,
    days,
    categories: [
      { categoryId: "research", label: "Research", counts: { planned: 2, completed: 1, carried: 1, cancelled: 0, pending: 0 } },
      { categoryId: "admin", label: "Admin", counts: { planned: 1, completed: 0, carried: 0, cancelled: 1, pending: 0 } },
      { categoryId: null, label: "Uncategorized", counts: { planned: 1, completed: 0, carried: 0, cancelled: 0, pending: 1 } },
    ],
    tasks,
  };
}

async function load() {
  loading.value = true;
  try {
    report.value = props.nativeBridgeAvailable
      ? await invoke<ReviewReport>("review_report", {
          period: period.value,
          anchorDate: anchorDate.value,
        })
      : browserReport(period.value, anchorDate.value);
    anchorDate.value = report.value.anchorDate;
  } catch (reason) {
    emit("error", String(reason));
  } finally {
    loading.value = false;
  }
}

function selectPeriod(value: ReviewPeriod) {
  if (period.value === value) return;
  period.value = value;
}

function move(amount: number) {
  if (!report.value || (amount > 0 && !canMoveForward.value)) return;
  anchorDate.value = shiftDate(report.value.anchorDate, period.value, amount);
  void load();
}

watch(period, () => void load());
onMounted(async () => {
  await load();
  await nextTick();
  closeButton.value?.focus();
});
</script>

<template>
  <section class="review-panel" role="dialog" aria-modal="true" aria-labelledby="review-title">
    <header class="review-header">
      <div>
        <p>{{ t("review.eyebrow") }}</p>
        <h2 id="review-title">{{ title }}</h2>
      </div>
      <button
        ref="closeButton"
        type="button"
        :aria-label="t('review.closeAriaLabel')"
        @click="emit('close')"
      >{{ t("review.close") }}</button>
    </header>

    <nav class="review-periods" :aria-label="t('review.periodAriaLabel')">
      <button
        v-for="item in periods"
        :key="item.value"
        type="button"
        :class="{ active: period === item.value }"
        :aria-pressed="period === item.value"
        @click="selectPeriod(item.value)"
      >
        {{ t(item.labelKey) }}
      </button>
    </nav>

    <div v-if="report" class="review-body" :aria-busy="loading">
      <div class="review-date-navigation">
        <button type="button" :aria-label="t('review.previousPeriod')" @click="move(-1)">←</button>
        <p>{{ periodLabel(report) }}</p>
        <button
          type="button"
          :aria-label="t('review.nextPeriod')"
          :disabled="!canMoveForward"
          @click="move(1)"
        >→</button>
      </div>

      <dl class="review-totals">
        <div v-for="row in countRows" :key="row.key">
          <dt>{{ t(row.labelKey) }}</dt>
          <dd>{{ report.totals[row.key] }}</dd>
        </div>
      </dl>

      <section v-if="period === 'daily'" class="review-detail" aria-labelledby="review-task-heading">
        <h3 id="review-task-heading">{{ t("review.taskFactsHeading") }}</h3>
        <p v-if="!report.tasks.length" class="review-empty">{{ t("review.noTasksForDay") }}</p>
        <ol v-else class="review-task-list">
          <li v-for="task in report.tasks" :key="task.id">
            <span class="review-status-mark" :data-status="task.status" aria-hidden="true"></span>
            <span>
              <strong>{{ task.title }}</strong>
              <small>{{ taskFacts(task) }}</small>
            </span>
          </li>
        </ol>
      </section>

      <template v-else>
        <section class="review-detail" aria-labelledby="review-days-heading">
          <h3 id="review-days-heading">{{ t("review.byTaskDayHeading") }}</h3>
          <p v-if="!activeDays.length" class="review-empty">{{ t("review.noTasksInPeriod") }}</p>
          <ol v-else class="review-fact-list">
            <li v-for="day in activeDays" :key="day.date">
              <strong>{{ shortDate(day.date) }}</strong>
              <span>{{ t("review.plannedCount", { count: day.counts.planned }) }}</span>
              <small>{{ statusFacts(day.counts) }}</small>
            </li>
          </ol>
        </section>

        <section class="review-detail" aria-labelledby="review-category-heading">
          <h3 id="review-category-heading">{{ t("review.byCategoryHeading") }}</h3>
          <p v-if="!report.categories.length" class="review-empty">{{ t("review.noCategories") }}</p>
          <ol v-else class="review-fact-list">
            <li v-for="category in report.categories" :key="category.categoryId ?? 'uncategorized'">
              <strong>{{ category.label }}</strong>
              <span>{{ t("review.plannedCount", { count: category.counts.planned }) }}</span>
              <small>{{ statusFacts(category.counts) }}</small>
            </li>
          </ol>
        </section>
      </template>
    </div>
    <p v-else-if="loading" class="review-loading" role="status">{{ t("review.loading") }}</p>
  </section>
</template>
