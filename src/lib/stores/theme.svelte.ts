import { createContext } from 'svelte';
import { settingsApi } from '$lib/api';

export type ThemePreference = 'light' | 'dark' | 'system';

export class ThemeState {
  theme = $state<ThemePreference>('system');
  
  isDark = $derived.by(() => {
    if (this.theme === 'system') {
      if (typeof window !== 'undefined' && window.matchMedia) {
        return window.matchMedia('(prefers-color-scheme: dark)').matches;
      }
      return false;
    }
    return this.theme === 'dark';
  });

  constructor(initialTheme?: ThemePreference) {
    if (initialTheme) {
      this.theme = initialTheme;
    } else {
      if (typeof window !== 'undefined') {
        const stored = localStorage.getItem('theme') as ThemePreference | null;
        if (stored === 'light' || stored === 'dark' || stored === 'system') {
          this.theme = stored;
        }
      }
      
      this.loadFromBackend();
    }
    
    this.applyTheme();
    
    if (typeof window !== 'undefined' && window.matchMedia) {
      window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
        if (this.theme === 'system') {
          this.applyTheme();
        }
      });
    }
  }
  
  private async loadFromBackend() {
    try {
      const backendTheme = await settingsApi.getTheme();
      if (backendTheme !== this.theme) {
        this.theme = backendTheme;
        this.applyTheme();
        if (typeof window !== 'undefined') {
          localStorage.setItem('theme', backendTheme);
        }
      }
    } catch (err) {
      console.error('Failed to load theme from backend:', err);
    }
  }
  
  async setTheme(newTheme: ThemePreference) {
    try {
      await settingsApi.setTheme(newTheme);
      this.theme = newTheme;
      
      if (typeof window !== 'undefined') {
        localStorage.setItem('theme', newTheme);
      }
      
      this.applyTheme();
    } catch (err) {
      console.error('Failed to set theme:', err);
      throw err;
    }
  }
  
  private applyTheme() {
    if (typeof window === 'undefined' || typeof document === 'undefined') {
      return;
    }
    
    const isDark = this.isDark;
    
    document.documentElement.classList.toggle('dark', isDark);
    document.documentElement.style.colorScheme = isDark ? 'dark' : 'light';
  }
}

export const [getThemeState, setThemeState] = createContext<ThemeState>();

export const createThemeState = (initialTheme?: ThemePreference) => {
  const theme = new ThemeState(initialTheme);
  setThemeState(theme);
  return theme;
};
