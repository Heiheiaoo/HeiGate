// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { defineComponent, ref, nextTick } from 'vue'
import { mount } from '@vue/test-utils'
import { vFocusTrap } from '@/directives/focusTrap'

// A dialog rendered with v-if, matching how every modal in GatewayView works.
const Host = defineComponent({
  directives: { focusTrap: vFocusTrap },
  setup() {
    const open = ref(false)
    return { open }
  },
  template: `
    <div>
      <button id="opener">open</button>
      <div v-if="open" v-focus-trap role="dialog">
        <button id="first">first</button>
        <button id="middle">middle</button>
        <button id="last">last</button>
      </div>
    </div>
  `,
})

function pressTab(shift = false) {
  document.dispatchEvent(
    new KeyboardEvent('keydown', { key: 'Tab', shiftKey: shift, bubbles: true, cancelable: true }),
  )
}

describe('v-focus-trap', () => {
  it('moves focus into the dialog when it opens', async () => {
    const wrapper = mount(Host, { attachTo: document.body })
    const opener = document.getElementById('opener') as HTMLElement
    opener.focus()
    expect(document.activeElement).toBe(opener)

    wrapper.vm.open = true
    await nextTick()

    expect(document.activeElement?.id).toBe('first')
    wrapper.unmount()
  })

  it('wraps from the last focusable back to the first', async () => {
    const wrapper = mount(Host, { attachTo: document.body })
    wrapper.vm.open = true
    await nextTick()

    ;(document.getElementById('last') as HTMLElement).focus()
    pressTab()
    expect(document.activeElement?.id).toBe('first')
    wrapper.unmount()
  })

  it('wraps backwards from the first to the last', async () => {
    const wrapper = mount(Host, { attachTo: document.body })
    wrapper.vm.open = true
    await nextTick()

    ;(document.getElementById('first') as HTMLElement).focus()
    pressTab(true)
    expect(document.activeElement?.id).toBe('last')
    wrapper.unmount()
  })

  it('leaves Tab alone in the middle of the dialog', async () => {
    const wrapper = mount(Host, { attachTo: document.body })
    wrapper.vm.open = true
    await nextTick()

    const middle = document.getElementById('middle') as HTMLElement
    middle.focus()
    pressTab()
    // Not at an edge: the browser's own tab order is left untouched.
    expect(document.activeElement?.id).toBe('middle')
    wrapper.unmount()
  })

  it('restores focus to the opener when the dialog closes', async () => {
    const wrapper = mount(Host, { attachTo: document.body })
    const opener = document.getElementById('opener') as HTMLElement
    opener.focus()

    wrapper.vm.open = true
    await nextTick()
    expect(document.activeElement?.id).toBe('first')

    wrapper.vm.open = false
    await nextTick()
    expect(document.activeElement).toBe(opener)
    wrapper.unmount()
  })

  it('stops trapping Tab once the dialog is gone', async () => {
    const wrapper = mount(Host, { attachTo: document.body })
    wrapper.vm.open = true
    await nextTick()
    wrapper.vm.open = false
    await nextTick()

    // The document-level listener must have been removed with the dialog;
    // dispatching Tab afterwards must not throw or move focus.
    const opener = document.getElementById('opener') as HTMLElement
    opener.focus()
    pressTab()
    expect(document.activeElement).toBe(opener)
    wrapper.unmount()
  })
})
