/**
 * Theme Store - Manages light/dark/system theme preference
 * Syncs with config.toml and localStorage for instant early access
 */

import { createContext } from 'svelte';
import { settingsApi } from '$lib/api';

export type ThemePreference = 'light' | 'dark' | 'system';

export class ThemeState {
  theme = $state<ThemePreference>('system');
  
  // Derived state: is the app currently in dark mode?
  isDark = $derived.by(() => {
    if (this.theme === 'system') {
      // Check system preference
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
      // Load from localStorage first (for instant access)
      if (typeof window !== 'undefined') {
        const stored = localStorage.getItem('theme') as ThemePreference | null;
        if (stored === 'light' || stored === 'dark' || stored === 'system') {
          this.theme = stored;
        }
      }
      
      // Then load from backend config.toml
      this.loadFromBackend();
    }
    
    // Apply theme class immediately
    this.applyTheme();
    
    // Listen for system preference changes
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
        // Sync to localStorage
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
      // Update backend first
      await settingsApi.setTheme(newTheme);
      
      // Then update local state
      this.theme = newTheme;
      
      // Sync to localStorage for early access on next launch
      if (typeof window !== 'undefined') {
        localStorage.setItem('theme', newTheme);
      }
      
      // Apply the theme
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
    
    // Toggle dark class on html element (Tailwind expects this)
    document.documentElement.classList.toggle('dark', isDark);
    
    // Set color-scheme for native scrollbars/form controls
    document.documentElement.style.colorScheme = isDark ? 'dark' : 'light';
  }
}

export const [getThemeState, setThemeState] = createContext<ThemeState>();

export const createThemeState = (initialTheme?: ThemePreference) => {
  const theme = new ThemeState(initialTheme);
  setThemeState(theme);
  return theme;
};
