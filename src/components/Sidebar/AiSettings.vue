<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useAiStore } from '../../stores/ai'
import { api } from '../../api'
import type { AiConfig, AiModel, AiProtocol, AiProvider } from '../../types'

const ai = useAiStore()
const draft = ref<AiConfig>({ enabled: false, providers: [], activeProviderId: '', activeModelId: '' })
const editingId = ref('')
const preset = ref('openai')
const saving = ref(false)
const testing = ref(false)
const fetching = ref(false)
const loading = ref(true)
const loadFailed = ref(false)
const error = ref('')
const feedback = ref('')
const modelId = ref('')
const modelName = ref('')
const modelVendor = ref('')
const vendors = [
  { id: 'openai', name: 'OpenAI', protocol: 'openai-responses', url: 'https://api.openai.com/v1' },
  { id: 'anthropic', name: 'Anthropic', protocol: 'anthropic', url: 'https://api.anthropic.com/v1' },
  { id: 'google', name: 'Google', protocol: 'gemini', url: 'https://generativelanguage.googleapis.com/v1beta' },
  { id: 'deepseek', name: 'DeepSeek', protocol: 'openai-chat', url: '' },
  { id: 'qwen', name: '阿里云 / 通义千问', protocol: 'openai-chat', url: '' },
  { id: 'moonshot', name: 'Moonshot / Kimi', protocol: 'openai-chat', url: '' },
  { id: 'zhipu', name: '智谱 / GLM', protocol: 'openai-chat', url: '' },
  { id: 'siliconflow', name: '硅基流动', protocol: 'openai-chat', url: '' },
  { id: 'ollama', name: 'Ollama / 本地模型', protocol: 'openai-chat', url: 'http://localhost:11434/v1' },
  { id: 'custom', name: '自定义 / 中转服务', protocol: 'openai-chat', url: '' },
] as const
const protocols: { id: AiProtocol; name: string; hint: string }[] = [
  { id: 'openai-chat', name: 'OpenAI Chat Completions', hint: '基础地址包含 /v1（如服务要求），系统追加 /chat/completions。' },
  { id: 'openai-responses', name: 'OpenAI Responses', hint: '基础地址包含 /v1（如服务要求），系统追加 /responses。' },
  { id: 'anthropic', name: 'Anthropic Messages', hint: '例如 https://api.anthropic.com/v1，系统追加 /messages。' },
  { id: 'gemini', name: 'Gemini GenerateContent', hint: '例如 https://generativelanguage.googleapis.com/v1beta，系统追加模型请求路径。' },
]
const provider = computed(() => draft.value.providers.find(p => p.id === editingId.value))
const activeProvider = computed(() => draft.value.providers.find(p => p.id === draft.value.activeProviderId))
const protocolHint = computed(() => protocols.find(p => p.id === provider.value?.protocol)?.hint)

async function load() {
  loading.value = true
  error.value = ''
  await ai.loadConfig()
  loadFailed.value = !!ai.error
  if (ai.error) error.value = ai.error
  draft.value = JSON.parse(JSON.stringify(ai.config)) as AiConfig
  editingId.value = draft.value.activeProviderId || draft.value.providers[0]?.id || ''
  loading.value = false
}
onMounted(load)

function addProvider() {
  const entry = vendors.find(v => v.id === preset.value) ?? vendors[0]
  const id = crypto.randomUUID()
  draft.value.providers.push({ id, name: entry.name, vendor: entry.id, protocol: entry.protocol,
    baseUrl: entry.url, apiKey: '', models: [] })
  selectProvider(id)
}
function selectProvider(id: string) {
  editingId.value = id
  modelId.value = modelName.value = modelVendor.value = ''
  feedback.value = error.value = ''
}
function setActiveProvider() {
  const p = activeProvider.value
  draft.value.activeModelId = p?.models[0]?.id ?? ''
}
function removeProvider() {
  const id = editingId.value
  draft.value.providers = draft.value.providers.filter(p => p.id !== id)
  if (draft.value.activeProviderId === id) {
    draft.value.activeProviderId = ''
    draft.value.activeModelId = ''
    draft.value.enabled = false
  }
  selectProvider(draft.value.providers[0]?.id ?? '')
}
function addModel() {
  const p = provider.value
  const id = modelId.value.trim()
  if (!p || !id) return
  if (p.models.some(m => m.id === id)) { error.value = '该 Provider 已包含此模型 ID'; return }
  p.models.push({ id, name: modelName.value.trim() || id, vendor: modelVendor.value.trim() })
  if (!draft.value.activeProviderId) {
    draft.value.activeProviderId = p.id
    draft.value.activeModelId = id
  } else if (draft.value.activeProviderId === p.id && !draft.value.activeModelId) {
    draft.value.activeModelId = id
  }
  modelId.value = modelName.value = modelVendor.value = ''
  error.value = ''
}
function updateModelId(model: AiModel, event: Event) {
  const id = (event.target as HTMLInputElement).value
  if (draft.value.activeProviderId === provider.value?.id && draft.value.activeModelId === model.id) {
    draft.value.activeModelId = id
  }
  model.id = id
}
function removeModel(model: AiModel) {
  const p = provider.value
  if (!p) return
  p.models = p.models.filter(m => m !== model)
  if (draft.value.activeProviderId === p.id && draft.value.activeModelId === model.id) {
    draft.value.activeModelId = p.models[0]?.id ?? ''
    if (!p.models.length) { draft.value.activeProviderId = ''; draft.value.enabled = false }
  }
}
async function save() {
  saving.value = true
  error.value = feedback.value = ''
  try {
    if (modelId.value.trim()) throw new Error('请先添加填写中的模型，或清空模型 ID')
    await ai.saveConfig(JSON.parse(JSON.stringify(draft.value)) as AiConfig)
    feedback.value = '配置已保存，AI 对话和定时任务规划将使用当前模型。'
  } catch (e) { error.value = String(e) }
  finally { saving.value = false }
}
async function fetchModels() {
  const p = provider.value
  if (!p) return
  fetching.value = true
  error.value = feedback.value = ''
  try {
    const models = await api.aiListModels(JSON.parse(JSON.stringify(p)) as AiProvider)
    let added = 0
    for (const model of models) {
      if (!p.models.some(m => m.id === model.id)) {
        p.models.push({ id: model.id, name: model.name, vendor: model.vendor })
        added += 1
      }
    }
    if (!draft.value.activeProviderId) {
      draft.value.activeProviderId = p.id
      draft.value.activeModelId = p.models[0]?.id ?? ''
    } else if (draft.value.activeProviderId === p.id && !draft.value.activeModelId) {
      draft.value.activeModelId = p.models[0]?.id ?? ''
    }
    feedback.value = `已获取 ${models.length} 个模型，新增 ${added} 个。`
  } catch (e) { error.value = String(e) }
  finally { fetching.value = false }
}
async function testConnection() {
  const p = provider.value
  if (!p) return
  testing.value = true
  error.value = feedback.value = ''
  try {
    const config = JSON.parse(JSON.stringify(draft.value)) as AiConfig
    config.enabled = true
    config.activeProviderId = p.id
    config.activeModelId = draft.value.activeProviderId === p.id ? draft.value.activeModelId : p.models[0]?.id ?? ''
    // 只验证所编辑的接入，其他未填完的草稿不影响测试。
    config.providers = [JSON.parse(JSON.stringify(p))]
    feedback.value = await api.aiTestConnection(config)
  } catch (e) { error.value = String(e) }
  finally { testing.value = false }
}
</script>

<template>
  <div class="ai-settings">
    <p class="intro">管理多个接入服务，每个 Provider 可使用独立的地址、协议、密钥和模型列表。</p>
    <p v-if="loading" class="hint">正在读取配置…</p>
    <div v-else-if="loadFailed" class="error" role="alert">{{ error }}<button @click="load">重新读取配置</button></div>
    <fieldset v-else class="config-fields" :disabled="saving || testing || fetching">
      <section class="ai-card">
        <div class="section-head"><h3>当前使用</h3><label class="enable"><input v-model="draft.enabled" type="checkbox" />启用 AI</label></div>
        <div class="field-grid">
          <label class="field"><span>Provider</span><select v-model="draft.activeProviderId" @change="setActiveProvider"><option value="">请选择接入服务</option><option v-for="p in draft.providers" :key="p.id" :value="p.id">{{ p.name }}</option></select></label>
          <label class="field"><span>模型</span><select v-model="draft.activeModelId" :disabled="!activeProvider"><option value="">请选择模型</option><option v-for="m in activeProvider?.models ?? []" :key="m.id" :value="m.id">{{ m.name || m.id }}</option></select></label>
        </div>
        <p class="hint">关闭 AI 时使用离线规则解析。密钥可留空，用于无需认证的本地服务。</p>
      </section>
      <section class="ai-card">
        <div class="section-head"><h3>接入服务</h3><span class="count">{{ draft.providers.length }} 个 Provider</span></div>
        <div class="add-provider"><select v-model="preset" aria-label="新 Provider 厂商"><option v-for="v in vendors" :key="v.id" :value="v.id">{{ v.name }}</option></select><button @click="addProvider">添加 Provider</button></div>
        <div v-if="draft.providers.length" class="provider-list" aria-label="Provider 列表">
          <button v-for="p in draft.providers" :key="p.id" class="provider-chip" :class="{ selected: p.id === editingId }" :aria-pressed="p.id === editingId" @click="selectProvider(p.id)">{{ p.name }}<span v-if="p.id === draft.activeProviderId" class="badge">当前</span></button>
        </div>
        <p v-else class="hint">添加一个 Provider，然后填写模型 ID。</p>
        <div v-if="provider" class="provider-editor">
          <div class="field-grid">
            <label class="field"><span>接入名称</span><input v-model="provider.name" placeholder="例如：公司网关 / 本地模型" /></label>
            <label class="field"><span>服务厂商</span><select v-model="provider.vendor"><option v-for="v in vendors" :key="v.id" :value="v.id">{{ v.name }}</option></select></label>
          </div>
          <label class="field"><span>接口协议</span><select v-model="provider.protocol"><option v-for="p in protocols" :key="p.id" :value="p.id">{{ p.name }}</option></select></label>
          <label class="field"><span>API 基础地址</span><input v-model="provider.baseUrl" type="url" placeholder="https://your-provider.example/v1" spellcheck="false" /><small>{{ protocolHint }}</small></label>
          <label class="field"><span>API Key</span><input v-model="provider.apiKey" type="password" placeholder="本地服务无需认证时可留空" autocomplete="off" spellcheck="false" /></label>
          <div class="section-head model-head"><h3>模型列表</h3><div class="model-head-actions"><span class="count">{{ provider.models.length }} 个模型</span><button :disabled="fetching || !provider.baseUrl.trim()" :title="provider.baseUrl.trim() ? '从该接入拉取可用模型' : '请先填写 API 基础地址'" @click="fetchModels">{{ fetching ? '拉取中…' : '拉取模型列表' }}</button></div></div>
          <p class="hint">模型 ID 按服务商实际提供的值填写；中转服务可添加不同厂商的模型。AI 文件操作需要模型支持工具调用。</p>
          <div v-for="(m, index) in provider.models" :key="index" class="model-row">
            <label class="field"><span>模型 ID</span><input :value="m.id" spellcheck="false" @input="updateModelId(m, $event)" /></label>
            <label class="field"><span>显示名称</span><input v-model="m.name" /></label>
            <label class="field"><span>模型厂商</span><input v-model="m.vendor" placeholder="可选" /></label>
            <button class="danger" :aria-label="`移除模型 ${m.name || m.id}`" @click="removeModel(m)">移除</button>
          </div>
          <div class="model-add">
            <div class="field-grid"><label class="field"><span>新模型 ID</span><input v-model="modelId" placeholder="服务商提供的模型 ID" spellcheck="false" @keydown.enter="addModel" /></label><label class="field"><span>显示名称</span><input v-model="modelName" placeholder="可选，默认使用模型 ID" /></label></div>
            <div class="add-provider"><input v-model="modelVendor" aria-label="新模型厂商" placeholder="模型厂商（可选）" /><button :disabled="!modelId.trim()" @click="addModel">添加模型</button></div>
          </div>
          <div class="provider-actions"><button :disabled="testing || !provider.models.length" @click="testConnection">{{ testing ? '测试中…' : '测试连接' }}</button><button class="danger" @click="removeProvider">移除 Provider</button></div>
          <p class="hint">测试会发送一条简短请求，可能产生少量接口费用，不发送文件内容。</p>
        </div>
      </section>
      <div class="save-bar"><p v-if="error" class="error" role="alert">{{ error }}</p><p v-else-if="feedback" class="success" role="status">{{ feedback }}</p><button class="primary" @click="save">{{ saving ? '保存中…' : '保存 AI 配置' }}</button></div>
    </fieldset>
  </div>
</template>

<style scoped>
.ai-settings { display: flex; flex-direction: column; gap: 16px; min-width: 0; }
.config-fields { display: flex; flex-direction: column; gap: 16px; min-width: 0; margin: 0; padding: 0; border: 0; }
.intro { margin: 0; color: var(--fg-muted); line-height: 1.6; }
.ai-card { display: flex; flex-direction: column; gap: 14px; padding: 16px; background: var(--bg-elevated); border: 1px solid var(--border); border-radius: var(--radius); min-width: 0; }
.section-head { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
h3 { margin: 0; font-size: 1.0833rem; font-weight: 600; }
.count, .hint, small { color: var(--fg-dim); font-size: 0.9167rem; line-height: 1.6; }
.hint { margin: 0; }
.enable { display: flex; align-items: center; gap: 6px; }
.enable input { accent-color: var(--accent); }
.field-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; }
.field { display: flex; flex-direction: column; gap: 6px; min-width: 0; color: var(--fg-muted); }
input:not([type='checkbox']), select { width: 100%; min-width: 0; min-height: 34px; padding: 6px 9px; font: inherit; color: var(--fg); background: var(--bg-input); border: 1px solid var(--border-strong); border-radius: var(--radius-sm); }
button { flex: none; min-height: 32px; padding: 6px 12px; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); color: var(--fg); }
button:hover:not(:disabled) { background: var(--bg-hover); }
button:disabled { opacity: 0.45; }
.primary { background: var(--accent); color: var(--accent-fg); }
.primary:hover:not(:disabled) { background: var(--accent); filter: brightness(0.9); }
.add-provider { display: flex; gap: 8px; align-items: center; }
.provider-list { display: flex; flex-wrap: wrap; gap: 8px; }
.provider-chip { display: flex; align-items: center; gap: 8px; max-width: 100%; overflow-wrap: anywhere; }
.provider-chip.selected { background: var(--accent-soft); border-color: var(--accent); }
.badge { color: var(--fg-muted); font-size: 0.8333rem; }
.provider-editor { display: flex; flex-direction: column; gap: 14px; padding-top: 14px; border-top: 1px solid var(--border); }
.model-head { padding-top: 8px; }
.model-head-actions { display: flex; align-items: center; gap: 8px; }
.model-head-actions button { min-height: 28px; padding: 4px 10px; font-size: 0.8333rem; }
.model-row { display: grid; grid-template-columns: minmax(0, 1.4fr) minmax(0, 1fr) minmax(0, 0.8fr) auto; gap: 8px; align-items: end; padding-bottom: 12px; border-bottom: 1px solid var(--border); }
.model-add { display: flex; flex-direction: column; gap: 10px; padding: 12px; border-radius: var(--radius-sm); background: var(--bg-hover); }
.provider-actions { display: flex; justify-content: space-between; gap: 8px; }
.danger { color: var(--danger); }
.save-bar { position: sticky; bottom: -20px; display: flex; align-items: center; justify-content: flex-end; gap: 12px; padding: 12px 0; border-top: 1px solid var(--border); background: var(--bg-panel); }
.error, .success { flex: 1; min-width: 0; margin: 0; line-height: 1.5; overflow-wrap: anywhere; }
.error { color: var(--danger); }
.success { color: var(--ok); }
@media (max-width: 720px) { .field-grid { grid-template-columns: minmax(0, 1fr); } .model-row { grid-template-columns: minmax(0, 1fr) auto; } .model-row .field:first-child { grid-column: 1 / -1; } .model-row .field:nth-child(3) { grid-column: 1; } .model-row button { grid-column: 2; grid-row: 2 / 4; } .add-provider { flex-wrap: wrap; } }
</style>
