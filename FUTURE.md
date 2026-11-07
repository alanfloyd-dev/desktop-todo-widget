# Future work

This document records the product direction and the planned path to the
next major release. It is a roadmap, not a specification or a promise
that every idea will ship. Keep implementation details and historical
release records in their dedicated documents; do not turn this file into
a changelog or an unprioritized feature wishlist.

## Product direction

Desktop Todo Widget should become a polished, lightweight desktop
companion that people genuinely want to keep visible on their desktop.

The goal is not simply to add more features or make screenshots look
impressive. The product should make a strong first impression while
remaining comfortable and useful during everyday, long-term use: clear
information hierarchy, coherent visual language, restrained motion,
predictable interactions, and low visual noise.

The next major milestone is v2.0.0. After that release, the project
moves into a maintenance-focused phase rather than continuing open-ended
feature expansion.

## Roadmap to v2.0.0

### 1. Finish the Settings panel

Status: In progress

Complete the current Settings-panel restructuring and resolve the known
behavior gaps before moving on to the product-wide visual refresh.

Scope:

- Finish the remaining Settings-panel component extraction where it
  improves ownership and maintainability.
- Keep draft state, dirty-state calculation, save behavior,
  navigation, language preview, asset cleanup, and deep-link handling
  under clear ownership.
- Preserve existing persistence and IPC contracts unless a separately
  reviewed change is required.
- Keep responsive navigation, keyboard behavior, contrast, and asset
  lifecycle covered by automated and native QA.
- Fix the known updater-install failure case so the user receives a
  visible error and can retry.

Current checkpoint (2026-11-07):

- S1-B/C/D: completed and pushed to origin, Gitee, and GitLab.
- S1-E1: AboutPanel extracted in local commit 82a0673; not yet
  pushed.
- E1.1: updater install-failure message fix implemented and independently reviewed; committed locally as 70a3228 (pending push).
- E2 (WeatherPanel) and the remaining Settings work: not started.

Exit condition: the Settings surface has clear component boundaries,
its existing behavior is preserved, known user-visible defects in scope
are fixed, and the integrated panel passes the agreed test and native-QA
gates.

### 2. Refresh the product-wide UI

Status: Planned

After the Settings work is complete, give the application a coherent,
product-level visual refresh.

Focus:

- Establish a clear visual direction and consistent design rules for
  typography, color, spacing, surfaces, icons, and component states.
- Improve the overall layout and visual hierarchy across the main
  widget and its modules.
- Refine interaction feedback and motion without making the interface
  distracting.
- Make appearance options feel intentional and consistent rather than
  like isolated effects.
- Validate the result in the actual desktop window, across supported
  window modes, practical sizes, DPI settings, and relevant light/dark
  or wallpaper conditions.

The design target is a product that feels distinctive at first glance
but remains calm and comfortable to leave on screen all day. Visual
effects should serve readability and usability, not compete with them.

### 3. Improve the Review experience

Status: Planned

Improve how Review presents a user's progress and phase summaries.

Focus:

- Make daily, weekly, and monthly Review easier to scan and
  understand.
- Give phase summaries a stronger information hierarchy.
- Present completed work, progress, and useful highlights clearly.
- Improve the visual presentation of existing Review information
  before expanding the underlying analytics.

This phase is primarily about making the existing Review experience more
useful and presentable. New scoring systems, speculative productivity
judgments, or AI-generated conclusions are not prerequisites for v2.0.0.

### 4. Stabilize and release v2.0.0

Status: Planned

Once Settings, the product-wide UI, and Review are ready, freeze the
v2.0.0 feature scope and focus on release quality.

Release work should include:

- Integrated regression testing across the main widget, modules,
  Settings, and Review.
- Persistence, upgrade compatibility, and user-data integrity checks.
- Real-machine validation of installation, update, recovery, and
  uninstall flows.
- Performance, resource-use, startup/shutdown, and
  long-running-session checks.
- Localization, responsive layout, accessibility, and error-state
  review.
- Updated documentation, release notes, and release artifacts.

Do not keep adding features during release hardening unless a change is
necessary to fix a release-blocking defect.

### 5. Produce the product video

Status: Planned

Create a polished promotional/demo video based on the release candidate
or final v2.0.0 build.

The video should show the product as it is actually used:

- Its presence and appearance on the desktop.
- The everyday task workflow.
- The available modules and their interactions.
- Review and phase-summary presentation.
- Personalization through Settings.

The video should communicate why someone would want to keep the widget
on their desktop. It should showcase real product behavior and avoid
implying capabilities that are not shipped.

### 6. Move into maintenance

Status: Planned after v2.0.0

After v2.0.0 and the product video are released, move the project into a
maintenance-focused phase.

Default priorities:

- Bug fixes and regressions.
- Security and dependency updates.
- Windows, WebView2, Rust, and Tauri compatibility.
- Installation, updater, recovery, and data-integrity maintenance.
- Documentation and release-pipeline upkeep.
- Small usability improvements when they address a concrete problem.

Major feature development is no longer the default. New features should
be considered separately, with a clear user need and an explicit scope
decision, rather than being added continuously to the release plan.

## Deferred ideas

The previous idea list contained many possibilities without a committed
priority. Those ideas should not distract from the v2.0.0 path.

Advanced weather products, expanded task-management workflows, AI
features, cloud accounts or synchronization, downloadable theme
marketplaces, calendar and notification integrations, advanced window
behavior, and richer Quick Links functionality remain uncommitted
ideas. They are not part of the v2.0.0 scope unless explicitly brought
back for review.

Keep newly discovered ideas in issues or a separate backlog. Add them
here only when they become a deliberate product direction.

## Engineering principles

- Preserve user data and compatibility; migrations and update paths
  are product-critical behavior.
- Keep the native/window-management layer stable by default. Change it
  for a concrete defect, platform/dependency compatibility, or a clear
  product requirement — not merely for architectural tidiness or
  visual novelty.
- Keep UI modules and component boundaries explicit, with clear
  ownership of state and side effects.
- Prefer focused, reviewable changes with tests and native validation
  over broad rewrites.
- Treat v2.0.0 as a product milestone, not a reason to expand scope
  indefinitely.
