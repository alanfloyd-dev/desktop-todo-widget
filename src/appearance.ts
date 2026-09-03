import type {
  AppearanceSettings,
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
  sampledLuminance: null,
};

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

export function browserResolvedContrast(
  mode: TextContrast,
  luminance: number | null,
  previous: ResolvedContrast | null,
): ResolvedContrast {
  if (mode === "light" || mode === "dark") return mode;
  const value = luminance ?? 0.16;
  if (previous === "light" && value < 0.62) return "light";
  if (previous === "dark" && value > 0.48) return "dark";
  return value >= 0.56 ? "dark" : "light";
}

export function browserRepresentativeLuminance(
  appearance: AppearanceSettings,
  sampled: number | null,
) {
  const hexLuminance = (value: string) => {
    const match = /^#([0-9a-f]{6})$/i.exec(value);
    if (!match) return 0.08;
    const encoded = match[1]!;
    const linear = (offset: number) => {
      const channel = Number.parseInt(encoded.slice(offset, offset + 2), 16) / 255;
      return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * linear(0) + 0.7152 * linear(2) + 0.0722 * linear(4);
  };
  let raw = sampled ?? 0.16;
  if (appearance.backgroundType === "solid") raw = hexLuminance(appearance.solidColor);
  if (appearance.backgroundType === "gradient") {
    raw = (hexLuminance(appearance.gradientStartColor) + hexLuminance(appearance.gradientEndColor)) / 2;
  }
  if (appearance.backgroundType === "glass") {
    const tint = hexLuminance(appearance.glassTintColor);
    raw = raw * (1 - appearance.glassTintOpacity) + tint * appearance.glassTintOpacity;
  }
  return raw * appearance.backgroundOpacity + 0.08 * (1 - appearance.backgroundOpacity);
}
