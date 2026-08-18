/// Theme tokens — the native mirror of mobile-web's index.css color variables
/// (Linear cool-grey dark + warm-paper light, Anthropic coral accent).
///
/// Reactivity model: the *resolved* theme ('light' | 'dark') lives in AppStorage
/// under `resolvedTheme`; components subscribe with `@StorageProp('resolvedTheme')`
/// and derive their token set via `themeTokens()`. The user's *setting*
/// ('system' | 'light' | 'dark') is persisted under `themeMode`. EntryAbility
/// feeds the system dark/light flag in (and refreshes it on configuration
/// changes), since only the ability context knows the OS color mode.

export type ThemeMode = 'system' | 'light' | 'dark';
export type ResolvedTheme = 'light' | 'dark';

const KEY_MODE = 'themeMode';
const KEY_SYSTEM_DARK = 'systemDark';
const KEY_RESOLVED = 'resolvedTheme';

/// The full token palette one resolved theme resolves to. Names mirror the
/// `--color-*` CSS custom properties one-to-one.
export interface ColorTokens {
  bg: string;
  bgSecondary: string;
  bgCard: string;
  bgCardHover: string;
  bgInput: string;
  text: string;
  textSecondary: string;
  textDim: string;
  border: string;
  borderStrong: string;
  accent: string;
  accentHover: string;
  accentSoft: string;
  success: string;
  warning: string;
  error: string;
  bgGlass: string;
}

// Translucent tokens use #AARRGGBB (ArkUI does not parse CSS rgba() strings).
const DARK: ColorTokens = {
  bg: '#08090a',
  bgSecondary: '#18191b',
  bgCard: '#101113',
  bgCardHover: '#18191b',
  bgInput: '#18191b',
  text: '#f7f8f8',
  textSecondary: '#8a8f98',
  textDim: '#62666d',
  border: '#14ffffff',
  borderStrong: '#26ffffff',
  accent: '#d97757',
  accentHover: '#c96040',
  accentSoft: '#2ed97757',
  success: '#4ade80',
  warning: '#fbbf24',
  error: '#f87171',
  bgGlass: '#eb08090a',
};

const LIGHT: ColorTokens = {
  bg: '#f1efea',
  bgSecondary: '#e7e4dd',
  bgCard: '#fbfaf7',
  bgCardHover: '#f1eee7',
  bgInput: '#fbfaf7',
  text: '#1f2023',
  textSecondary: '#5d6168',
  textDim: '#8d9199',
  border: '#1c201c12',
  borderStrong: '#38201c12',
  accent: '#c25232',
  accentHover: '#a94527',
  accentSoft: '#21c25232',
  success: '#178042',
  warning: '#9a6a08',
  error: '#cc3340',
  bgGlass: '#ebf1efea',
};

/// Token set for a resolved theme string. Defaults to dark for any unknown value.
export function themeTokens(resolved: string): ColorTokens {
  return resolved === 'light' ? LIGHT : DARK;
}

export function getThemeMode(): ThemeMode {
  const m = AppStorage.get<string>(KEY_MODE);
  if (m === 'light' || m === 'dark' || m === 'system') {
    return m as ThemeMode;
  }
  return 'system';
}

/// Recompute `resolvedTheme` from the current mode + system flag. Called on any
/// change so `@StorageProp('resolvedTheme')` subscribers re-render.
export function applyResolvedTheme(): void {
  const mode = getThemeMode();
  const systemDark = AppStorage.get<boolean>(KEY_SYSTEM_DARK) ?? true;
  const resolved: ResolvedTheme = mode === 'system' ? (systemDark ? 'dark' : 'light') : mode;
  AppStorage.setOrCreate(KEY_RESOLVED, resolved);
}

export function setThemeMode(mode: ThemeMode): void {
  AppStorage.setOrCreate(KEY_MODE, mode);
  applyResolvedTheme();
}

/// Called from EntryAbility.onCreate (and onConfigurationUpdate) with the OS
/// dark-mode flag. Persists the mode setting across launches and resolves once.
export function initTheme(systemDark: boolean): void {
  PersistentStorage.persistProp(KEY_MODE, 'system');
  AppStorage.setOrCreate(KEY_SYSTEM_DARK, systemDark);
  applyResolvedTheme();
}

/// Update just the system dark flag (configuration change) and re-resolve.
export function setSystemDark(systemDark: boolean): void {
  AppStorage.setOrCreate(KEY_SYSTEM_DARK, systemDark);
  applyResolvedTheme();
}
