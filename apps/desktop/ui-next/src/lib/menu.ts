// The menu model shared by every menu in the app. Items are plain data, so a builder can return them and the menu
// components render them with the shadcn DropdownMenu. Ported from ui/views/contextmenu.js.
//
//   { label, icon, leading, sub, danger, disabled, onPick }   an item; onPick gets the click (Shift+click skips a confirm)
//   { label, submenu: [...] }                                 an item that opens a submenu beside its row
//   { label, radio: true, checked }                           a radio row, with the mark on the right
//   { slider: { label, min, max, step, value, format, onInput, onChange } }   a range row; Left and Right change it
//   { header: 'text' }                                        a heading that the keys skip
//   'sep'                                                     a separator
//   null, false, undefined                                    nothing (so a builder can write `cond ? item : null`)
import type { KeyboardEvent, MouseEvent, ReactNode } from 'react';
import type { LucideIcon } from 'lucide-react';
import { create } from 'zustand';

export interface MenuItem {
  label: string;
  icon?: LucideIcon;
  leading?: ReactNode;
  sub?: string;
  danger?: boolean;
  disabled?: boolean;
  radio?: boolean;
  checked?: boolean;
  submenu?: MenuEntry[];
  onPick?: (e: MouseEvent<HTMLElement> | KeyboardEvent<HTMLElement>) => void;
}

export interface MenuHeader {
  header: string;
}

export interface MenuSlider {
  slider: {
    label: string;
    min: number;
    max: number;
    step?: number;
    value: number;
    format?: (n: number) => string;
    onInput?: (n: number) => void;
    onChange?: (n: number) => void;
  };
}

export type MenuEntry = MenuItem | MenuHeader | MenuSlider | 'sep' | null | undefined | false;

export interface OpenMenu {
  id: number;
  x: number;
  y: number;
  items: MenuEntry[];
  placeAbove: boolean;
  opener: HTMLElement | null; // the element that had the focus, which gets it back when the menu closes
}

interface MenuState {
  menu: OpenMenu | null;
}

export const useContextMenu = create<MenuState>(() => ({ menu: null }));

let seq = 0;

// Opens the menu with its top-left corner at (x, y), or its bottom-left corner when `placeAbove`. The menu keeps itself
// inside the window. A menu opened while another is open replaces it.
export function openContextMenu(x: number, y: number, items: MenuEntry[], { placeAbove = false }: { placeAbove?: boolean } = {}): void {
  const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  useContextMenu.setState({ menu: { id: ++seq, x, y, items, placeAbove, opener } });
}

export function closeContextMenu(): void {
  useContextMenu.setState({ menu: null });
}
