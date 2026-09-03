<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type {
  ReviewCounts,
  ReviewPeriod,
  ReviewReport,
  ReviewTask,
} from "../types";

const props = defineProps<{ nativeBridgeAvailable: boolean }>();
const emit = defineEmits<{ close: []; error: [message: string] }>();

const period = ref<ReviewPeriod>("daily");
const anchorDate = ref<string | null>(null);
const report = ref<ReviewReport | null>(null);
const loading = ref(true);
const closeButton = ref<HTMLButtonElement>();

const periods: Array<{ value: ReviewPeriod; label: string }> = [
  { value: "daily", label: "Daily" },
  { value: "weekly", label: "Weekly" },
  { value: "monthly", label: "Monthly" },
];
const countRows: Array<{ key: keyof ReviewCounts; label: string }> = [
  { key: "planned", label: "Planned" },
  { key: "completed", label: "Completed" },
  { key: "carried", label: "Carried" },
  { key: "cancelled", label: "Cancelled" },
  { key: "pending", label: "Pending" },
];
const shortMonths = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
const longMonths = [
  "JANUARY", "FEBRUARY", "MARCH", "APRIL", "MAY", "JUNE",
  "JULY", "AUGUST", "SEPTEMBER", "OCTOBER", "NOVEMBER", "DECEMBER",
];
const weekdays = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

const title = computed(() => `${period.value.toUpperCase()} REVIEW`);
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
  const date = utcDate(value);
  return `${String(date.getUTCDate()).padStart(2, "0")} ${shortMonths[date.getUTCMonth()]} ${date.getUTCFullYear()}`;
}

function shortDate(value: string) {
  const date = utcDate(value);
  return `${weekdays[date.getUTCDay()]} ${String(date.getUTCDate()).padStart(2, "0")} ${shortMonths[date.getUTCMonth()]}`;
}

function periodLabel(value: ReviewReport) {
  if (value.period === "daily") return longDate(value.anchorDate);
  if (value.period === "monthly") {
    const date = utcDate(value.periodStart);
    return `${longMonths[date.getUTCMonth()]} ${date.getUTCFullYear()}`;
  }
  return `${longDate(value.periodStart)} — ${longDate(value.periodEnd)}`;
}

function statusFacts(counts: ReviewCounts) {
  const facts: string[] = [];
  if (counts.completed) facts.push(`${counts.completed} completed`);
  if (counts.carried) facts.push(`${counts.carried} carried`);
  if (counts.cancelled) facts.push(`${counts.cancelled} cancelled`);
  if (counts.pending) facts.push(`${counts.pending} pending`);
  return facts.length ? facts.join(" · ") : "No tasks planned";
}

function taskStatus(task: ReviewTask) {
  return task.status.charAt(0).toUpperCase() + task.status.slice(1);
}

function taskFacts(task: ReviewTask) {
  const facts = [taskStatus(task)];
  if (task.completedAt !== null) {
    facts.push(new Intl.DateTimeFormat(undefined, {
      hour: "2-digit",
      minute: "2-digit",
    }).format(new Date(task.completedAt * 1000)));
  }
  if (task.categoryName) facts.push(task.categoryName);
  if (task.carriedFrom) facts.push("Carried forward");
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
        <p>REVIEW / REPORTS</p>
        <h2 id="review-title">{{ title }}</h2>
      </div>
      <button ref="closeButton" type="button" aria-label="Close review" @click="emit('close')">Close</button>
    </header>

    <nav class="review-periods" aria-label="Review period">
      <button
        v-for="item in periods"
        :key="item.value"
        type="button"
        :class="{ active: period === item.value }"
        :aria-pressed="period === item.value"
        @click="selectPeriod(item.value)"
      >
        {{ item.label }}
      </button>
    </nav>

    <div v-if="report" class="review-body" :aria-busy="loading">
      <div class="review-date-navigation">
        <button type="button" aria-label="Previous review period" @click="move(-1)">←</button>
        <p>{{ periodLabel(report) }}</p>
        <button type="button" aria-label="Next review period" :disabled="!canMoveForward" @click="move(1)">→</button>
      </div>

      <dl class="review-totals">
        <div v-for="row in countRows" :key="row.key">
          <dt>{{ row.label }}</dt>
          <dd>{{ report.totals[row.key] }}</dd>
        </div>
      </dl>

      <section v-if="period === 'daily'" class="review-detail" aria-labelledby="review-task-heading">
        <h3 id="review-task-heading">TASK FACTS</h3>
        <p v-if="!report.tasks.length" class="review-empty">No tasks planned for this task day.</p>
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
          <h3 id="review-days-heading">BY TASK DAY</h3>
          <p v-if="!activeDays.length" class="review-empty">No tasks planned in this period.</p>
          <ol v-else class="review-fact-list">
            <li v-for="day in activeDays" :key="day.date">
              <strong>{{ shortDate(day.date) }}</strong>
              <span>{{ day.counts.planned }} planned</span>
              <small>{{ statusFacts(day.counts) }}</small>
            </li>
          </ol>
        </section>

        <section class="review-detail" aria-labelledby="review-category-heading">
          <h3 id="review-category-heading">BY CATEGORY</h3>
          <p v-if="!report.categories.length" class="review-empty">No categories represented in this period.</p>
          <ol v-else class="review-fact-list">
            <li v-for="category in report.categories" :key="category.categoryId ?? 'uncategorized'">
              <strong>{{ category.label }}</strong>
              <span>{{ category.counts.planned }} planned</span>
              <small>{{ statusFacts(category.counts) }}</small>
            </li>
          </ol>
        </section>
      </template>
    </div>
    <p v-else-if="loading" class="review-loading" role="status">Loading review…</p>
  </section>
</template>
