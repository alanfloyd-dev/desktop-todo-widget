<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { qaTrace } from "../qa-trace";

type TaskStatus = "pending" | "completed" | "cancelled" | "carried";

interface Task {
  id: string;
  title: string;
  categoryId: string | null;
  categoryName: string | null;
  status: TaskStatus;
  scheduledDate: string;
  completedAt: number | null;
  sortOrder: number;
  carriedFrom: string | null;
}

interface Category {
  id: string;
  name: string;
  sortOrder: number;
}

interface TodayState {
  taskDay: string;
  tasks: Task[];
  previousTaskDay: string;
  previousPending: Task[];
  categories: Category[];
  monthSummary: { month: string; completed: number; carried: number };
}

const props = defineProps<{ nativeBridgeAvailable: boolean }>();
const emit = defineEmits<{ error: [message: string]; openReview: [] }>();

const state = ref<TodayState>(browserTodayState());
const newTitle = ref("");
const newCategoryId = ref("");
const addInput = ref<HTMLInputElement>();
const editingId = ref<string | null>(null);
const editTitle = ref("");
const editCategoryId = ref("");
const categoryComposerOpen = ref(false);
const categoryName = ref("");
const categoryInput = ref<HTMLInputElement>();
const hiddenPrevious = ref(new Set<string>());
const draggedId = ref<string | null>(null);
const dragTargetId = ref<string | null>(null);
const dragPointerId = ref<number | null>(null);
let dragHandle: HTMLElement | null = null;

const pending = computed(() => state.value.tasks.filter((task) => task.status === "pending"));
const completed = computed(() => state.value.tasks.filter((task) => task.status === "completed"));
const history = computed(() =>
  state.value.tasks.filter((task) => task.status === "cancelled" || task.status === "carried"),
);
const reviewTasks = computed(() =>
  state.value.previousPending.filter((task) => !hiddenPrevious.value.has(task.id)),
);

function browserTaskDay() {
  const now = new Date();
  if (now.getHours() < 4) now.setDate(now.getDate() - 1);
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
}

function browserTodayState(): TodayState {
  const taskDay = browserTaskDay();
  return {
    taskDay,
    tasks: [],
    previousTaskDay: "",
    previousPending: [],
    categories: [],
    monthSummary: { month: taskDay.slice(0, 7), completed: 0, carried: 0 },
  };
}

function browserId(prefix: string) {
  return `${prefix}-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

async function load() {
  if (!props.nativeBridgeAvailable) return;
  try {
    state.value = await invoke<TodayState>("today_tasks");
  } catch (reason) {
    emit("error", String(reason));
  }
}

async function add() {
  const title = newTitle.value.trim();
  if (!title) {
    qaTrace("add_empty_title");
    return;
  }
  qaTrace("add_invoke_start");
  try {
    if (props.nativeBridgeAvailable) {
      await invoke("add_task", { title, categoryId: newCategoryId.value || null });
      qaTrace("add_invoke_ok");
      await load();
      qaTrace("add_reload_ok");
    } else {
      state.value.tasks.push({
        id: browserId("task"),
        title,
        categoryId: newCategoryId.value || null,
        categoryName:
          state.value.categories.find((category) => category.id === newCategoryId.value)?.name ?? null,
        status: "pending",
        scheduledDate: state.value.taskDay,
        completedAt: null,
        sortOrder: (pending.value.length + 1) * 10,
        carriedFrom: null,
      });
    }
    newTitle.value = "";
    await nextTick();
    addInput.value?.focus();
  } catch (reason) {
    qaTrace("add_invoke_err");
    emit("error", String(reason));
  }
}

/** Keyboard submit path; traced separately so a dead key path is visible. */
async function addSubmitFromKeyboard() {
  qaTrace("add_submit_enter");
  await add();
}

/** Temporary B3 diagnostic: records which key the add input actually received. */
function traceAddKey(event: KeyboardEvent) {
  qaTrace(`key:${event.key}`);
}

function beginEdit(task: Task) {
  editingId.value = task.id;
  editTitle.value = task.title;
  editCategoryId.value = task.categoryId ?? "";
  nextTick(() => document.querySelector<HTMLInputElement>(".task-editing input")?.focus());
}

function cancelEdit() {
  editingId.value = null;
}

async function saveEdit(task: Task) {
  const title = editTitle.value.trim();
  if (!title) return;
  try {
    if (props.nativeBridgeAvailable) {
      await invoke("edit_task", {
        id: task.id,
        title,
        categoryId: editCategoryId.value || null,
      });
      await load();
    } else {
      task.title = title;
      task.categoryId = editCategoryId.value || null;
      task.categoryName =
        state.value.categories.find((category) => category.id === editCategoryId.value)?.name ?? null;
    }
    editingId.value = null;
  } catch (reason) {
    emit("error", String(reason));
  }
}

async function toggleComplete(task: Task) {
  try {
    if (props.nativeBridgeAvailable) {
      await invoke("toggle_task_completed", { id: task.id });
      await load();
    } else {
      task.status = task.status === "completed" ? "pending" : "completed";
      task.completedAt = task.status === "completed" ? Math.floor(Date.now() / 1000) : null;
      refreshBrowserSummary();
    }
  } catch (reason) {
    emit("error", String(reason));
  }
}

async function cancelTask(task: Task, fromReview = false) {
  try {
    if (props.nativeBridgeAvailable) {
      await invoke("cancel_task", { id: task.id });
      await load();
    } else if (fromReview) {
      state.value.previousPending = state.value.previousPending.filter((item) => item.id !== task.id);
    } else {
      task.status = "cancelled";
    }
  } catch (reason) {
    emit("error", String(reason));
  }
}

async function carryTask(task: Task, fromReview = false) {
  try {
    if (props.nativeBridgeAvailable) {
      await invoke("carry_task", { id: task.id });
      await load();
    } else {
      task.status = "carried";
      if (fromReview) {
        state.value.previousPending = state.value.previousPending.filter((item) => item.id !== task.id);
        state.value.tasks.push({
          ...task,
          id: browserId("carried"),
          status: "pending",
          scheduledDate: state.value.taskDay,
          completedAt: null,
          carriedFrom: task.id,
          sortOrder: (pending.value.length + 1) * 10,
        });
      }
      refreshBrowserSummary();
    }
  } catch (reason) {
    emit("error", String(reason));
  }
}

async function removeTask(task: Task) {
  const historical = task.status === "completed" || task.status === "carried";
  if (historical && !window.confirm("Delete this historical task permanently?")) return;
  try {
    if (props.nativeBridgeAvailable) {
      await invoke("delete_task", { id: task.id, confirmHistorical: historical });
      await load();
    } else {
      state.value.tasks = state.value.tasks.filter((item) => item.id !== task.id);
      refreshBrowserSummary();
    }
  } catch (reason) {
    emit("error", String(reason));
  }
}

function keepPrevious(task: Task) {
  hiddenPrevious.value = new Set([...hiddenPrevious.value, task.id]);
}

async function createCategory() {
  const name = categoryName.value.trim();
  if (!name) return;
  try {
    let category: Category;
    if (props.nativeBridgeAvailable) {
      category = await invoke<Category>("create_category", { name });
      await load();
    } else {
      category = { id: browserId("category"), name, sortOrder: state.value.categories.length * 10 + 10 };
      state.value.categories.push(category);
    }
    newCategoryId.value = category.id;
    categoryName.value = "";
    categoryComposerOpen.value = false;
    await nextTick();
    addInput.value?.focus();
  } catch (reason) {
    emit("error", String(reason));
  }
}

function openCategoryComposer() {
  categoryComposerOpen.value = true;
  nextTick(() => categoryInput.value?.focus());
}

function dndLog(message: string) {
  if (import.meta.env.DEV) console.debug(`[todo-dnd] ${message}`);
}

// WebView2 did not reliably promote draggable rows into an HTML5 dragstart,
// especially with buttons and editors inside the row. A dedicated handle plus
// pointer capture keeps reorder independent from both those controls and the
// Tauri window drag region; the database mutation remains the same transaction.
function dragPointerDown(task: Task, event: PointerEvent) {
  if (event.button !== 0) return;
  qaTrace("drag_pointerdown");
  draggedId.value = task.id;
  dragTargetId.value = task.id;
  dragPointerId.value = event.pointerId;
  dragHandle = event.currentTarget as HTMLElement;
  dragHandle.setPointerCapture(event.pointerId);
  dndLog(`pointerdown source=${task.id} button=${event.button}`);
}

function taskAtPointer(event: PointerEvent) {
  return document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>(".pending-row")?.dataset.taskId;
}

function dragPointerMove(event: PointerEvent) {
  if (dragPointerId.value !== event.pointerId || !draggedId.value) return;
  event.preventDefault();
  const targetId = taskAtPointer(event);
  if (!targetId) return;
  if (targetId !== dragTargetId.value) {
    qaTrace("drag_pointermove");
    dndLog(`dragenter target=${targetId}`);
  }
  dragTargetId.value = targetId;
  dndLog(`dragover target=${targetId}`);
}

function dragEnd(event?: PointerEvent) {
  if (event && dragPointerId.value === event.pointerId && dragHandle?.hasPointerCapture(event.pointerId)) {
    dragHandle.releasePointerCapture(event.pointerId);
  }
  dndLog(`dragend source=${draggedId.value ?? "none"}`);
  draggedId.value = null;
  dragTargetId.value = null;
  dragPointerId.value = null;
  dragHandle = null;
}

async function dragPointerUp(event: PointerEvent) {
  if (event.button !== 0 || dragPointerId.value !== event.pointerId) return;
  event.preventDefault();
  const sourceId = draggedId.value;
  const targetId = taskAtPointer(event) || dragTargetId.value;
  if (!sourceId || !targetId || sourceId === targetId) {
    dragEnd(event);
    return;
  }
  qaTrace("drag_pointerup");
  dndLog(`drop source=${sourceId} target=${targetId}`);
  const ordered = [...pending.value];
  const from = ordered.findIndex((task) => task.id === sourceId);
  const to = ordered.findIndex((task) => task.id === targetId);
  if (from < 0 || to < 0) {
    dragEnd(event);
    return;
  }
  const [moved] = ordered.splice(from, 1);
  ordered.splice(to, 0, moved);
  try {
    if (props.nativeBridgeAvailable) {
      qaTrace("drag_reorder_start");
      await invoke("reorder_tasks", { ids: ordered.map((task) => task.id) });
      qaTrace("drag_reorder_ok");
      await load();
    } else {
      const historical = state.value.tasks.filter((task) => task.status !== "pending");
      ordered.forEach((task, index) => (task.sortOrder = (index + 1) * 10));
      state.value.tasks = [...ordered, ...historical];
    }
  } catch (reason) {
    qaTrace("drag_reorder_err");
    emit("error", String(reason));
  } finally {
    dragEnd(event);
  }
}

function refreshBrowserSummary() {
  state.value.monthSummary.completed = state.value.tasks.filter((task) => task.status === "completed").length;
  state.value.monthSummary.carried = state.value.tasks.filter((task) => task.status === "carried").length;
}

onMounted(() => {
  window.addEventListener("pointermove", dragPointerMove, { passive: false });
  window.addEventListener("pointerup", dragPointerUp);
  window.addEventListener("pointercancel", dragEnd);
  void load();
});

onBeforeUnmount(() => {
  window.removeEventListener("pointermove", dragPointerMove);
  window.removeEventListener("pointerup", dragPointerUp);
  window.removeEventListener("pointercancel", dragEnd);
});
</script>

<template>
  <section class="product-section today-section" aria-labelledby="today-title">
    <div class="section-heading">
      <h1 id="today-title">TODAY</h1>
      <div class="section-heading-actions">
        <span>{{ state.taskDay }}</span>
        <button type="button" class="review-entry" @click="emit('openReview')">Review</button>
      </div>
    </div>

    <div v-if="reviewTasks.length" class="previous-review">
      <p>{{ reviewTasks.length }} unfinished from {{ state.previousTaskDay }}</p>
      <div v-for="task in reviewTasks" :key="task.id" class="review-row">
        <span>{{ task.title }}</span>
        <div>
          <button type="button" @click="carryTask(task, true)">Carry</button>
          <button type="button" @click="keepPrevious(task)">Keep previous</button>
          <button type="button" @click="cancelTask(task, true)">Cancel</button>
        </div>
      </div>
    </div>

    <div class="task-list" aria-live="polite">
      <article
        v-for="task in pending"
        :key="task.id"
        class="task-row pending-row"
        :data-task-id="task.id"
      >
        <span
          class="task-drag-handle"
          aria-label="Reorder task"
          title="Drag to reorder"
          @pointerdown="dragPointerDown(task, $event)"
        >⋮⋮</span>
        <button class="task-check" type="button" aria-label="Complete task" @click="toggleComplete(task)"></button>
        <template v-if="editingId === task.id">
          <div class="task-editing">
            <input
              v-model="editTitle"
              aria-label="Task title"
              @keydown.enter.prevent="saveEdit(task)"
              @keydown.esc.prevent="cancelEdit"
            />
            <select v-model="editCategoryId" aria-label="Task category">
              <option value="">No category</option>
              <option v-for="category in state.categories" :key="category.id" :value="category.id">
                {{ category.name }}
              </option>
            </select>
          </div>
        </template>
        <div v-else class="task-copy" @dblclick="beginEdit(task)">
          <span>{{ task.title }}</span>
          <small v-if="task.categoryName">{{ task.categoryName }}</small>
        </div>
        <div class="task-actions">
          <button type="button" @click="beginEdit(task)">Edit</button>
          <button type="button" @click="carryTask(task)">Carry</button>
          <button type="button" @click="cancelTask(task)">Cancel</button>
          <button type="button" @click="removeTask(task)">Delete</button>
        </div>
      </article>

      <article v-for="task in completed" :key="task.id" class="task-row task-completed">
        <button class="task-check checked" type="button" aria-label="Reopen task" @click="toggleComplete(task)">✓</button>
        <div class="task-copy">
          <span>{{ task.title }}</span>
          <small v-if="task.categoryName">{{ task.categoryName }}</small>
        </div>
        <div class="task-actions">
          <button type="button" @click="removeTask(task)">Delete</button>
        </div>
      </article>
    </div>

    <p v-if="!state.tasks.length" class="todo-empty">Nothing scheduled for this task day.</p>

    <form class="task-add" @submit.prevent="add">
      <span aria-hidden="true">＋</span>
      <input
        ref="addInput"
        v-model="newTitle"
        aria-label="New task"
        placeholder="Add a task"
        @keydown="traceAddKey"
        @keydown.enter.prevent="addSubmitFromKeyboard"
        @click="qaTrace('add_submit_click')"
      />
      <select v-model="newCategoryId" aria-label="New task category">
        <option value="">No category</option>
        <option v-for="category in state.categories" :key="category.id" :value="category.id">
          {{ category.name }}
        </option>
      </select>
      <button type="button" class="category-add" aria-label="Create category" @click="openCategoryComposer">Category +</button>
      <!--
        B3: the add form previously had no submit control, so pressing Enter in the
        field was the only way to create a task. When that keystroke was missed the
        user had no alternative and no visible affordance. This is a real, labelled
        submit button; the form's existing @submit handler performs the add.
      -->
      <button type="submit" class="category-add add-submit" aria-label="Add task" @click="qaTrace('add_submit_click')">Add</button>
    </form>

    <form v-if="categoryComposerOpen" class="category-composer" @submit.prevent="createCategory">
      <input
        ref="categoryInput"
        v-model="categoryName"
        aria-label="Category name"
        placeholder="Category name"
        @keydown.enter.prevent="createCategory"
        @keydown.esc.prevent="categoryComposerOpen = false"
      />
      <button type="submit">Create</button>
      <button type="button" @click="categoryComposerOpen = false">Cancel</button>
    </form>

    <details v-if="history.length" class="task-history">
      <summary>History · {{ history.length }}</summary>
      <article v-for="task in history" :key="task.id" class="task-row history-row">
        <span class="history-mark">{{ task.status === "carried" ? "→" : "×" }}</span>
        <div class="task-copy">
          <span>{{ task.title }}</span>
          <small>{{ task.status }}</small>
        </div>
        <div class="task-actions">
          <button type="button" @click="removeTask(task)">Delete</button>
        </div>
      </article>
    </details>
  </section>

  <section class="product-section month-section" aria-labelledby="month-title">
    <h2 id="month-title">MONTH</h2>
    <p>
      {{ state.monthSummary.month }} · {{ state.monthSummary.completed }} completed ·
      {{ state.monthSummary.carried }} carried
    </p>
  </section>
</template>
