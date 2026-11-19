import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import { i18n } from './i18n'
import { installNativeShell } from './shell/native'
import '@xterm/xterm/css/xterm.css'
import './styles/tokens.css'
import './styles/global.css'

installNativeShell()

createApp(App).use(createPinia()).use(i18n).mount('#app')
