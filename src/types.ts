export type ProductWindowMode = "sidebar" | "floating" | "desktop";
export type SidebarSide = "left" | "right";
export type TemperatureUnit = "celsius" | "fahrenheit";
export type FloatingPresentation = "collapsed" | "expanded";
export type BackgroundType = "glass" | "solid" | "gradient" | "image" | "wallpaper";
export type TextContrast = "auto" | "light" | "dark" | "custom";
export type ResolvedContrast = "light" | "dark";
export type ImageFit = "cover" | "contain" | "stretch";
export type ImagePosition = "center" | "top" | "bottom";
export type ReviewPeriod = "daily" | "weekly" | "monthly";
export type TaskStatus = "pending" | "completed" | "cancelled" | "carried";
/**
 * Product UI language preference.
 *
 * `system` follows the operating system locale; the other two force a
 * language. v1 supports English and Simplified Chinese only.
 */
export type Language = "system" | "en" | "zh-Hans";

export interface ReviewCounts {
  planned: number;
  completed: number;
  carried: number;
  cancelled: number;
  pending: number;
}

export interface ReviewDay {
  date: string;
  counts: ReviewCounts;
}

export interface ReviewCategory {
  categoryId: string | null;
  label: string;
  counts: ReviewCounts;
}

export interface ReviewTask {
  id: string;
  title: string;
  categoryName: string | null;
  status: TaskStatus;
  scheduledDate: string;
  completedAt: number | null;
  carriedFrom: string | null;
}

export interface ReviewReport {
  period: ReviewPeriod;
  anchorDate: string;
  periodStart: string;
  periodEnd: string;
  currentTaskDay: string;
  totals: ReviewCounts;
  days: ReviewDay[];
  categories: ReviewCategory[];
  tasks: ReviewTask[];
}

export interface AppearanceSettings {
  backgroundType: BackgroundType;
  solidColor: string;
  glassTintColor: string;
  glassTintOpacity: number;
  blurPx: number;
  overlayStrength: number;
  gradientStartColor: string;
  gradientEndColor: string;
  gradientAngle: number;
  imageAssetId: string | null;
  imageFit: ImageFit;
  imagePosition: ImagePosition;
  backgroundOpacity: number;
  textContrast: TextContrast;
  /** Product text colour, used when `textContrast` is `custom`. */
  customTextColor: string;
  sampledLuminance: number | null;
}

/**
 * Appearance per window mode.
 *
 * Sidebar, Floating, and Desktop each own a complete `AppearanceSettings`, so
 * editing one mode's appearance never changes another's, and switching mode
 * selects that mode's own profile.
 */
export interface AppearanceProfiles {
  sidebar: AppearanceSettings;
  floating: AppearanceSettings;
  desktop: AppearanceSettings;
}
export type WeatherCondition =
  | "clear"
  | "mainly-clear"
  | "partly-cloudy"
  | "cloudy"
  | "fog"
  | "drizzle"
  | "rain"
  | "snow"
  | "showers"
  | "thunderstorm"
  | "unknown";
export type WeatherCacheStatus = "fresh" | "stale" | "very-stale" | "missing";

export interface ProductSettings {
  geometryUnitsVersion: number;
  mode: ProductWindowMode;
  x: number | null;
  y: number | null;
  width: number;
  height: number;
  monitorIdentity: string | null;
  floatingPresentation: FloatingPresentation;
  floatingOrbX: number | null;
  floatingOrbY: number | null;
  floatingOrbMonitorIdentity: string | null;
  desktopX: number | null;
  desktopY: number | null;
  desktopWidth: number;
  desktopHeight: number;
  sidebarSide: SidebarSide;
  sidebarWidth: number;
  alwaysOnTop: boolean;
  locked: boolean;
  dayRollover: string;
  weatherLocationLabel: string;
  weatherLatitude: number | null;
  weatherLongitude: number | null;
  weatherTimezone: string;
  weatherCountry: string;
  weatherAdmin1: string;
  temperatureUnit: TemperatureUnit;
  language: Language;
  appearance: string;
  appearanceProfiles: AppearanceProfiles;
  displayName: string;
  avatarAssetId: string | null;
  /**
   * Quick Links, in display order.
   *
   * `id` is the stable identity used by edit/delete/reorder, so the array index is
   * never the identity. Names and URLs are user data and are never localized.
   */
  quickLinks: QuickLink[];
}

export interface QuickLink {
  id: string;
  name: string;
  url: string;
}

export interface AssetPayload {
  assetId: string | null;
  available: boolean;
  dataUrl: string | null;
}

export interface ContrastResult {
  resolved: ResolvedContrast;
  representativeLuminance: number;
}

export interface LocationCandidate {
  label: string;
  latitude: number;
  longitude: number;
  timezone: string;
  country: string;
  admin1: string;
}

export interface WeatherSnapshot {
  condition: WeatherCondition;
  temperature: number;
  dailyHigh: number;
  dailyLow: number;
  precipitationProbability: number;
  fetchedAt: number;
  locationKey: string;
  timezone: string;
  temperatureUnit: TemperatureUnit;
}

export interface WeatherViewState {
  configured: boolean;
  snapshot: WeatherSnapshot | null;
  cacheStatus: WeatherCacheStatus;
  cacheAgeSeconds: number | null;
  refreshRecommended: boolean;
  refreshing: boolean;
  lastRefresh: string;
}

export interface ProductViewState {
  settings: ProductSettings;
  desktopExperimental: boolean;
  databasePath: string;
  /**
   * Raw operating system locale, used to resolve the `system` language
   * preference. Empty when the platform value could not be read.
   */
  systemLocale: string;
}

export interface DesktopDiagnostics {
  hwnd: string;
  parentHwnd: string;
  parentClass: string;
  shellStrategy: string;
  style: string;
  exStyle: string;
  currentDesktopHwnd: string;
  isWindowVisible: boolean;
  attachmentValid: boolean;
  recoveryCount: number;
  recoveryReason: string;
  x: number;
  y: number;
  width: number;
  height: number;
  attach: {
    status: string;
    hwnd: string;
    parentBefore: string;
    parentTarget: string;
    parentAfter: string;
    styleBefore: string;
    styleAfter: string;
    exStyleBefore: string;
    exStyleAfter: string;
    boundsBefore: string;
    boundsAfterParent: string;
    boundsFinal: string;
    desktopTargetRect: string;
    desktopClientRect: string;
    tauriScaleFactor: string;
    windowDpi: number;
    logicalBounds: string;
    physicalRequested: string;
  };
  events: string[];
}
