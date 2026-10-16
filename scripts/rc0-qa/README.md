# RC0/RC1 QA tooling (v1.2.0 release-candidate rounds)

Ad-hoc QA automation used during the desktop-todo-widget v1.2.0 RC-0/RC-1
release-candidate QA rounds: PowerShell UI-automation and screen-capture
probes, Node `.mjs` frontend drivers, and small Python helpers for seeding
and inspecting persisted product settings.

This directory is **QA tooling only**:

- It is not a production runtime component and not a build dependency —
  nothing in the product, the installer, or the release pipeline reads it.
- Scripts may drive real product windows and persisted settings; they are
  for a QA machine, not for end users.
- The QA user/profile names that appear inside some scripts are test
  fixtures. Scripts contain no machine-specific absolute paths.
- Validation status when archived: PowerShell scripts pass parser checks,
  Python scripts pass `python -m py_compile`, and `.mjs` scripts pass
  `node --check`.

Keep this directory free of screenshots, logs, binaries, and
`__pycache__/` (delete local `__pycache__/` before staging). The QA round
results themselves are recorded in `docs/rc0-orb-sidebar-resize.md` and
[docs/release-signing.md](../../docs/release-signing.md).
