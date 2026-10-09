import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import { installNativeShell } from './shell/native'
import './styles/tokens.css'
import './styles/global.css'

installNativeShell()

createApp(App).use(createPinia()).mount('#app')
