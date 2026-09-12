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

export function profileInitials(displayName: string) {
  const words = displayName.trim().split(/\s+/).filter(Boolean);
  if (!words.length || displayName.trim().toLowerCase() === "your name") return "AD";
  const first = words[0]?.[0] ?? "";
  const last = words.length > 1 ? words[words.length - 1]?.[0] ?? "" : words[0]?.[1] ?? "";
  const initials = `${first}${last}`.toLocaleUpperCase();
  return initials || "AD";
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
