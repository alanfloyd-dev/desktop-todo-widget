import type {
  AppearanceProfiles,
  AppearanceSettings,
  ProductWindowMode,
  ResolvedContrast,
  TextContrast,
} from "./types";

export const DEFAULT_APPEARANCE: AppearanceSettings = {
  backgroundType: "glass",
  solidColor: "#11191e",
  glassTintColor: "#11191e",
  glassTintOpacity: 0.78,
  blurPx: 14,
  overlayStrength: 0.18,
  gradientStartColor: "#11191e",
  gradientEndColor: "#213747",
  gradientAngle: 135,
  imageAssetId: null,
  imageFit: "cover",
  imagePosition: "center",
  backgroundOpacity: 0.94,
  textContrast: "auto",
  // Matches the default graphite body text, so selecting Custom before picking a
  // colour does not change the surface.
  customTextColor: "#d3dade",
  sampledLuminance: null,
};

/** Every window mode, in the order the Settings profile selector lists them. */
export const APPEARANCE_MODES: ProductWindowMode[] = ["sidebar", "floating", "desktop"];

/**
 * Edge length of the persisted avatar, in pixels.
 *
 * The Orb renders it at 56 DIP and the expanded footer at 28 DIP, so 256 px keeps
 * it crisp at every Windows scaling level while the stored PNG stays a few tens of
 * kilobytes. Kept in step with `AVATAR_EDGE_PX` in `src-tauri/src/appearance.rs`,
 * which bounds what the backend will accept.
 */
export const AVATAR_EDGE_PX = 256;

/**
 * Normalizes a picked avatar into the small square the profile stores.
 *
 * A user picks a photo, not an icon: it can be a multi-megabyte, non-square,
 * EXIF-rotated original. This decodes it, applies its orientation, takes the
 * centre square — the same region the circular `object-fit: cover` preview shows —
 * scales it to {@link AVATAR_EDGE_PX}, and re-encodes it as PNG so transparency
 * survives. Only this result is persisted, so the profile never carries the
 * original, and the backend receives a payload whose format the app itself chose.
 *
 * The bytes are decoded from the data URL directly rather than fetched: the
 * product's CSP allows no `data:` connection source.
 *
 * Rejects with a message the caller turns into localized copy when the image
 * cannot be decoded at all (for example a truncated file).
 */
export async function normalizeAvatarImage(dataUrl: string): Promise<string> {
  const { bytes, mime } = dataUrlBytes(dataUrl);
  const blob = new Blob([bytes], { type: mime });
  let bitmap: ImageBitmap;
  try {
    // `from-image` honours the EXIF orientation tag, so a portrait photo taken on
    // a phone is not stored sideways.
    bitmap = await createImageBitmap(blob, { imageOrientation: "from-image" });
  } catch {
    throw new Error("selected image could not be read");
  }
  const edge = Math.min(bitmap.width, bitmap.height);
  if (!edge) {
    bitmap.close();
    throw new Error("selected image could not be read");
  }
  const canvas = document.createElement("canvas");
  canvas.width = AVATAR_EDGE_PX;
  canvas.height = AVATAR_EDGE_PX;
  const context = canvas.getContext("2d");
  if (!context) {
    bitmap.close();
    throw new Error("selected image could not be read");
  }
  context.imageSmoothingEnabled = true;
  context.imageSmoothingQuality = "high";
  context.drawImage(
    bitmap,
    Math.round((bitmap.width - edge) / 2),
    Math.round((bitmap.height - edge) / 2),
    edge,
    edge,
    0,
    0,
    AVATAR_EDGE_PX,
    AVATAR_EDGE_PX,
  );
  bitmap.close();
  return canvas.toDataURL("image/png");
}

/** Splits a base64 `data:` URL into its bytes and declared media type. */
function dataUrlBytes(dataUrl: string): { bytes: Uint8Array<ArrayBuffer>; mime: string } {
  const comma = dataUrl.indexOf(",");
  const header = comma < 0 ? "" : dataUrl.slice(0, comma);
  if (!header.startsWith("data:") || !header.endsWith(";base64")) {
    throw new Error("unsupported image");
  }
  const mime = header.slice("data:".length, header.length - ";base64".length);
  const binary = atob(dataUrl.slice(comma + 1));
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return { bytes, mime };
}

/** A fresh, independent appearance profile per window mode. */
export function defaultAppearanceProfiles(): AppearanceProfiles {
  return {
    sidebar: { ...DEFAULT_APPEARANCE },
    floating: { ...DEFAULT_APPEARANCE },
    desktop: { ...DEFAULT_APPEARANCE },
  };
}

/**
 * The appearance of the window mode the product is currently in.
 *
 * Every appearance consumer resolves through this, so a mode switch shows that
 * mode's own profile instead of carrying the previous look over.
 */
export function activeAppearance(settings: {
  mode: ProductWindowMode;
  appearanceProfiles: AppearanceProfiles;
}): AppearanceSettings {
  return settings.appearanceProfiles[settings.mode] ?? DEFAULT_APPEARANCE;
}

/**
 * CSS variables for a Custom text colour.
 *
 * Only the body and secondary text variables are replaced. Accent, warning/error,
 * task-state and status-mark colours keep their own values, so a custom colour
 * cannot make a task state or an error unreadable — it answers the background,
 * it does not repaint the meaning of the surface.
 */
export function customTextVariables(
  appearance: AppearanceSettings,
): Record<string, string> | undefined {
  if (appearance.textContrast !== "custom") return undefined;
  const color = /^#[0-9a-f]{6}$/i.test(appearance.customTextColor)
    ? appearance.customTextColor.toLowerCase()
    : DEFAULT_APPEARANCE.customTextColor;
  return {
    "--ink": color,
    "--muted": withAlpha(color, 0.62),
    "--faint": withAlpha(color, 0.42),
  };
}

function withAlpha(color: string, alpha: number): string {
  const channels = [1, 3, 5].map((offset) => Number.parseInt(color.slice(offset, offset + 2), 16));
  return `rgba(${channels.join(", ")}, ${alpha})`;
}

/**
 * Up to two initials derived from the user's display name.
 *
 * Rules, in order:
 * - two or more words: first letter of the first word + first letter of the last
 *   word (`Alan Floyd` -> `AF`);
 * - one word: that word's first letter only (`User` -> `U`). Taking its first two
 *   letters would render the neutral placeholder as `US`, which reads as a
 *   different identity rather than as an initial;
 * - empty or whitespace-only: no initials at all.
 *
 * Purely derived, and deliberately with no fallback identity: this function can
 * never invent a profile the user did not configure. The neutral default comes
 * from the persisted display name itself, not from a literal here — that is what
 * keeps the two in step when the user edits the name.
 */
export function profileInitials(displayName: string) {
  const words = displayName.trim().split(/\s+/).filter(Boolean);
  if (!words.length) return "";
  const first = words[0]?.[0] ?? "";
  if (words.length === 1) return first.toLocaleUpperCase();
  const last = words[words.length - 1]?.[0] ?? "";
  return `${first}${last}`.toLocaleUpperCase();
}

export async function sampleImageLuminance(dataUrl: string): Promise<number | null> {
  const image = new Image();
  image.decoding = "async";
  const loaded = new Promise<boolean>((resolve) => {
    image.onload = () => resolve(true);
    image.onerror = () => resolve(false);
  });
  image.src = dataUrl;
  if (!(await loaded)) return null;
  const canvas = document.createElement("canvas");
  canvas.width = 32;
  canvas.height = 32;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) return null;
  context.drawImage(image, 0, 0, canvas.width, canvas.height);
  const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
  let total = 0;
  let count = 0;
  const linear = (channel: number) => {
    const value = channel / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  for (let index = 0; index < pixels.length; index += 4) {
    const alpha = pixels[index + 3]! / 255;
    const luminance =
      0.2126 * linear(pixels[index]!) +
      0.7152 * linear(pixels[index + 1]!) +
      0.0722 * linear(pixels[index + 2]!);
    total += luminance * alpha + 0.08 * (1 - alpha);
    count += 1;
  }
  return count ? Math.max(0, Math.min(1, total / count)) : null;
}

/**
 * Browser mirror of the native `resolve_appearance_contrast` policy.
 *
 * The two have to agree: the browser preview and the packaged app must not
 * disagree about whether a surface is light-on-dark.
 */
export function browserResolvedContrast(
  appearance: AppearanceSettings,
  luminance: number | null,
  previous: ResolvedContrast | null,
): ResolvedContrast {
  const mode: TextContrast = appearance.textContrast;
  if (mode === "light" || mode === "dark") return mode;
  if (mode === "custom") {
    const custom = hexLuminance(appearance.customTextColor);
    // A bright custom colour is light text and keeps the light palette, so the
    // accent, semantic and overlay colours stay behind it. See the native
    // `custom_contrast`.
    if (custom !== null) return custom >= 0.5 ? "light" : "dark";
  }
  const value = luminance ?? 0.16;
  if (previous === "light" && value < 0.62) return "light";
  if (previous === "dark" && value > 0.48) return "dark";
  return value >= 0.56 ? "dark" : "light";
}

/** Relative luminance of an `#rrggbb` colour, or `null` when unparsable. */
function hexLuminance(value: string): number | null {
  const match = /^#([0-9a-f]{6})$/i.exec(value);
  if (!match) return null;
  const encoded = match[1]!;
  const linear = (offset: number) => {
    const channel = Number.parseInt(encoded.slice(offset, offset + 2), 16) / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * linear(0) + 0.7152 * linear(2) + 0.0722 * linear(4);
}

export function browserRepresentativeLuminance(
  appearance: AppearanceSettings,
  sampled: number | null,
) {
  const luminance = (value: string) => hexLuminance(value) ?? 0.08;
  let raw = sampled ?? 0.16;
  if (appearance.backgroundType === "solid") raw = luminance(appearance.solidColor);
  if (appearance.backgroundType === "gradient") {
    raw = (luminance(appearance.gradientStartColor) + luminance(appearance.gradientEndColor)) / 2;
  }
  if (appearance.backgroundType === "glass") {
    const tint = luminance(appearance.glassTintColor);
    raw = raw * (1 - appearance.glassTintOpacity) + tint * appearance.glassTintOpacity;
  }
  return raw * appearance.backgroundOpacity + 0.08 * (1 - appearance.backgroundOpacity);
}
