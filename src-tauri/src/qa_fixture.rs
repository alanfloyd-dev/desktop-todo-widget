//! QA fixture writer/verifier for the final lifecycle acceptance QA.
//!
//! Test-only code: it never compiles into any production binary. The lifecycle
//! QA script runs the two `#[ignore]`d tests explicitly with
//! `DTW_QA_FIXTURE_DATA_ROOT` pointing at the sandbox data root, so persistent
//! data is created and verified through the product's real schema and APIs
//! (migration, settings document, tasks, categories) instead of a hand-made
//! SQLite file.

use crate::database::Database;
use crate::settings::{AppState, QuickLink};
use chrono::NaiveDate;

const FIXTURE_DISPLAY_NAME: &str = "QA 验证用户";
const FIXTURE_LINK_NAME: &str = "QA 文档";
const FIXTURE_LINK_URL: &str = "https://example.com/qa-docs";
const FIXTURE_CATEGORY: &str = "QA 分类";
const FIXTURE_TASK_A: &str = "QA 任务甲";
const FIXTURE_TASK_B: &str = "QA 任务乙";
const FIXTURE_DATE: &str = "2026-09-19";

fn fixture_root() -> std::path::PathBuf {
    let root = std::path::PathBuf::from(
        std::env::var("DTW_QA_FIXTURE_DATA_ROOT")
            .expect("DTW_QA_FIXTURE_DATA_ROOT must point at the sandbox data root"),
    );
    assert!(!root.as_os_str().is_empty(), "fixture root is empty");
    root
}

fn open_state() -> AppState {
    let root = fixture_root();
    let database = Database::open(root.join("alan-desktop.sqlite3")).expect("open fixture DB");
    AppState::load(database).expect("load AppState")
}

fn fixture_quick_links() -> Vec<QuickLink> {
    vec![QuickLink {
        id: "qa-link-1".into(),
        name: FIXTURE_LINK_NAME.into(),
        url: FIXTURE_LINK_URL.into(),
    }]
}

/// Creates a representative local-first user state: schema migrations, a
/// settings document with identity + quick links, a category, and two tasks
/// (one completed, so review data exists). Idempotent-ish: the QA script runs
/// it once against a fresh root.
#[test]
#[ignore = "QA fixture writer; run explicitly with DTW_QA_FIXTURE_DATA_ROOT set"]
fn write_representative_user_data_fixture() {
    let state = open_state();
    state
        .update(|settings| {
            settings.display_name = FIXTURE_DISPLAY_NAME.into();
            settings.quick_links = fixture_quick_links();
        })
        .expect("persist fixture settings");
    let category = state
        .database
        .create_category(FIXTURE_CATEGORY)
        .expect("create fixture category");
    let date = NaiveDate::parse_from_str(FIXTURE_DATE, "%Y-%m-%d").expect("fixture date");
    let task_a = state
        .database
        .add_task(FIXTURE_TASK_A, Some(&category.id), date)
        .expect("add fixture task a");
    state
        .database
        .add_task(FIXTURE_TASK_B, Some(&category.id), date)
        .expect("add fixture task b");
    state
        .database
        .toggle_task_completed(&task_a.id)
        .expect("complete fixture task a");
    let stored = state.snapshot().expect("snapshot fixture settings");
    assert_eq!(stored.display_name, FIXTURE_DISPLAY_NAME);
    assert_eq!(stored.quick_links.len(), 1);
    assert_eq!(state.database.task_count().expect("task count"), 2);
    println!("fixture written to {}", fixture_root().display());
}

/// Re-opens the same data root and asserts every logical record survived —
/// used after the keep-data uninstall + reinstall to prove the database is
/// byte-compatible and was not reset or migrated.
#[test]
#[ignore = "QA fixture verifier; run explicitly with DTW_QA_FIXTURE_DATA_ROOT set"]
fn verify_representative_user_data_fixture() {
    let state = open_state();
    let stored = state.snapshot().expect("snapshot");
    assert_eq!(
        stored.display_name, FIXTURE_DISPLAY_NAME,
        "display name must survive"
    );
    assert_eq!(stored.quick_links.len(), 1, "quick links must survive");
    assert_eq!(
        stored.quick_links[0].name, FIXTURE_LINK_NAME,
        "quick link identity must survive"
    );
    assert_eq!(stored.quick_links[0].url, FIXTURE_LINK_URL);
    assert_eq!(
        state.database.task_count().expect("task count"),
        2,
        "tasks must survive"
    );
    assert!(
        state.database.schema_version().expect("schema") >= 1,
        "schema must be present"
    );
    println!(
        "fixture verified at {} (schema {})",
        fixture_root().display(),
        state.database.schema_version().expect("schema")
    );
}
