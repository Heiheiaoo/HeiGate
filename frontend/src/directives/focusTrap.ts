import { ref, type Directive } from 'vue'

// How many dialogs are currently mounted. Every modal is wrapped with this
// directive, so this is the one reliable "is any modal open" signal without
// having to OR together each modal's own flag. The window chrome uses it to
// re-expose a title-bar drag strip once a backdrop covers the normal one.
export const openModalCount = ref(0)

// Selector for elements that can hold keyboard focus inside a dialog. Anything
// explicitly removed from the tab order (tabindex="-1") is excluded.
const FOCUSABLE = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',')

// A control is skipped only when it is genuinely not rendered. Measuring
// geometry (offsetParent / getBoundingClientRect) is unreliable here: a
// position:fixed dialog reports a null offsetParent even when visible, so that
// test would skip every control the dialog holds.
function isVisible(el: HTMLElement): boolean {
  if (el.hidden || el.getAttribute('aria-hidden') === 'true') return false
  const style = getComputedStyle(el)
  return style.display !== 'none' && style.visibility !== 'hidden'
}

function focusableWithin(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(isVisible)
}

const handlers = new WeakMap<HTMLElement, (event: KeyboardEvent) => void>()
const restoreTo = new WeakMap<HTMLElement, HTMLElement | null>()

/**
 * Keeps keyboard focus inside an open dialog and restores it on close.
 *
 * Without this, Tab walks out of a modal into the page behind it: a keyboard or
 * screen-reader user ends up operating controls they cannot see while the modal
 * still covers them, and closing the modal drops focus to <body>, losing their
 * place.
 *
 * Implemented as a directive rather than a composable because every modal here
 * is rendered with `v-if` — mounted/unmounted already fire exactly on open and
 * close, so no extra ref or watcher is needed per modal.
 *
 * This directive owns Tab containment and focus restoration only; dialogs are
 * dismissed by their own close button or a backdrop click.
 */
export const vFocusTrap: Directive<HTMLElement> = {
  mounted(el) {
    openModalCount.value++
    restoreTo.set(el, document.activeElement as HTMLElement | null)

    const onKeydown = (event: KeyboardEvent) => {
      if (event.key !== 'Tab') return
      const focusable = focusableWithin(el)
      if (focusable.length === 0) {
        event.preventDefault()
        return
      }
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      const active = document.activeElement as HTMLElement | null

      if (event.shiftKey && (active === first || !el.contains(active))) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && active === last) {
        event.preventDefault()
        first.focus()
      }
    }

    handlers.set(el, onKeydown)
    document.addEventListener('keydown', onKeydown, true)

    // Move focus into the dialog so the screen reader announces it. Fall back to
    // the dialog element itself when it holds no focusable control.
    const focusable = focusableWithin(el)
    const target = focusable[0] ?? el
    if (target === el) el.tabIndex = -1
    target.focus()
  },

  unmounted(el) {
    openModalCount.value = Math.max(0, openModalCount.value - 1)
    const onKeydown = handlers.get(el)
    if (onKeydown) {
      document.removeEventListener('keydown', onKeydown, true)
      handlers.delete(el)
    }
    // Return the user to whatever opened the dialog.
    restoreTo.get(el)?.focus?.()
    restoreTo.delete(el)
  },
}
