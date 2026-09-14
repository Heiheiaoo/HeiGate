import { createApp } from 'vue'
import App from './App.vue'
import './style.css'

// Follow the OS appearance; the window chrome is drawn by the native shell.
const media = window.matchMedia('(prefers-color-scheme: dark)')
const applyTheme = (dark: boolean) => document.documentElement.classList.toggle('dark', dark)
applyTheme(media.matches)
media.addEventListener('change', (event) => applyTheme(event.matches))

createApp(App).mount('#app')
