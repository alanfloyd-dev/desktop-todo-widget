# Appearance and personalization

## Material architecture

Glass · Graphite Frost is the first-install default; the Phase 4 opaque graphite look remains selectable as Solid · Graphite. The Tauri window and WebView2 controller are transparent, and `html`, `body`, `#app`, the product root, and the material container remain transparent. The Vue material then adds a semi-transparent graphite tint/overlay below a fully opaque content layer. Solid forces its background layer to opacity 1.

On Windows, expanded Floating and Sidebar Glass use Tauri's native Acrylic effect behind the transparent WebView. CSS `backdrop-filter` remains a progressive enhancement, not the asserted cross-HWND blur path. Collapsed Orb disables Acrylic and the native window shadow so its transparent corners stay circular. Desktop clears the system effect before the existing WebView HWND becomes a `WS_CHILD` of `SHELLDLL_DefView`, then uses true transparency plus a translucent graphite tint; this avoids reopening the validated Phase 1 SetParent, input, Win+D, or geometry lifecycle. When a requested image or wallpaper is unavailable, the material falls back to the same transparent graphite Glass while the rest of the app stays usable.

## Backgrounds and local privacy

Supported background types are Glass, Solid, two-stop Gradient, managed Image, and the current Windows wallpaper. Image selection accepts PNG, JPEG, and WebP, validates both size and file signature, then copies the bytes into the platform app-data `assets` directory using a generated ID. It never modifies or later deletes the source file. Replacement/removal cleans only an unreferenced, recognized Alan Desktop managed asset.

The Windows wallpaper path is read from `HKCU\\Control Panel\\Desktop\\WallPaper` only when requested. Alan Desktop does not change wallpaper, poll the Registry, capture the screen, accept remote image URLs, or upload/analyze images remotely. Rust returns validated data URLs to the WebView, never native paths. Missing, oversized, unreadable, or corrupt data returns an unavailable result and the UI shows the fallback.

Profile avatars use the same managed-copy boundary with a lower size limit. No-avatar fallback derives up to two Unicode initials from the configured display name; an empty/unusable name gets a generic local `AD` mark. Reset Appearance intentionally leaves avatar, profile, Weather, Todo, task-day, modes, and geometry unchanged.

## Contrast and performance

Text contrast supports Auto, Light, and Dark. Auto uses relative luminance of a Solid color or the average of Gradient stops. Image/wallpaper backgrounds are sampled once after load/change through a 32×32 canvas; that representative value may be persisted with settings. Glass combines the sample/fallback with tint, opacity, and overlay. A hysteresis band around the threshold prevents light/dark oscillation. Blur is clamped to 0–24 px and there is no per-render sampling or desktop capture loop.

## Floating Avatar Orb

Floating remains one of exactly three window modes. Its presentation is either `collapsed` or `expanded`; it is not a fourth mode and does not create a second WebView/HWND. First-install settings are Floating + collapsed. The Orb is 56 logical pixels, keyboard focusable, left-clickable, right-clickable, and draggable when unlocked. A centralized 5 DIP pointer threshold separates click from drag.

The invariant is: Orb anchor and expanded Floating size are independent persisted values. Expansion uses the Orb's current monitor and grows inward from the nearest horizontal/vertical edge, then clamps inside the work area. Collapse reapplies the saved anchor instead of deriving it from the clamped expanded rectangle, so repeated transitions do not drift. Logical DIP values convert using the target monitor scale; 56 DIP is 84 physical pixels at 150%.

Sidebar and Desktop always show the full Widget. Mode transitions retain their existing independent geometry and topmost/lock semantics. Desktop remains a bounded interactive Shell child and forces always-on-top off.

## Diagnostics and limitations

Copied diagnostics may report background type, availability, requested/resolved contrast, avatar configured yes/no, Floating presentation, and logical Orb bounds. They never include profile text, homepage URL, managed IDs, asset filenames/paths, wallpaper path, image bytes, or sampled content.

Native Acrylic is asserted only for top-level expanded Floating and Sidebar windows. Desktop does not claim optical blur because the system backdrop is not reliable after Shell child reparenting; its documented fallback is translucent Graphite. Current-wallpaper mode is a separate rendered image background, not native transparency, and the wallpaper is reread on app/settings load rather than watched continuously. These constraints keep Desktop attachment stable and GPU/network/privacy cost bounded.
