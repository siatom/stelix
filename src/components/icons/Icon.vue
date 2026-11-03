<script setup lang="ts">
/** Shared icon component backed by Lucide, with legacy paths for app-specific marks. */
import { computed } from 'vue'
import {
  Archive, ArrowDown, ArrowDownUp, ArrowLeft, ArrowLeftRight, ArrowRight, ArrowUp, Bot, Check,
  CheckCircle, ChevronDown, ChevronLeft, ChevronRight, ChevronUp, CircleAlert,
  Computer,
  Clock, Cloud, Columns2, Command, Copy, Disc3, Download, ExternalLink, Eye,
  Code, File, FilePlus2, FileText, Filter, Folder, FolderOpen, FolderOutput, FolderPlus, FolderSymlink,
  Globe, HardDrive, Home, Image, Info, LayoutGrid, LayoutList, List, LoaderCircle,
  Megaphone, MessageSquare, Minus, Moon, MoreHorizontal, Music2, Network, PanelLeftClose,
  PanelLeftOpen, PanelLeft, Pencil, Play, Plus, RefreshCw, RotateCcw, Search,
  PanelsLeftRight, Rows3, Send, Settings, Sparkles, Square, Star, Sun, Terminal,
  Trash2, TriangleAlert, Upload, UserRound, Video, X, type LucideIcon,
} from '@lucide/vue'
import { ICONS, type IconName } from './paths'

const props = withDefaults(
  defineProps<{
    name: IconName
    size?: number | string
    strokeWidth?: number
  }>(),
  { size: 16, strokeWidth: 1.7 },
)

const libraryIcons: Partial<Record<IconName, LucideIcon>> = {
  'chevron-right': ChevronRight, 'chevron-down': ChevronDown,
  'chevron-left': ChevronLeft, 'chevron-up': ChevronUp,
  more: MoreHorizontal, 'arrow-left': ArrowLeft, 'arrow-right': ArrowRight,
  'arrow-up': ArrowUp, 'arrow-down': ArrowDown, refresh: RefreshCw,
  restore: RotateCcw, 'folder-plus': FolderPlus, pencil: Pencil, trash: Trash2,
  copy: Copy, swap: ArrowLeftRight, x: X, plus: Plus, minus: Minus, check: Check,
  'view-details': LayoutList, 'view-list': List, 'view-icons': LayoutGrid,
  sort: ArrowDownUp, filter: Filter, folder: Folder, 'folder-open': FolderOpen,
  'folder-arrow': FolderSymlink, file: File, 'file-plus': FilePlus2,
  'file-text': FileText, image: Image, video: Video, music: Music2,
  archive: Archive, code: Code, terminal: Terminal, package: Archive,
  disc: Disc3, drive: HardDrive, home: Home, star: Star, clock: Clock,
  user: UserRound, monitor: Computer, download: Download, upload: Upload,
  cloud: Cloud, network: Network, globe: Globe, settings: Settings,
  'panel-left-close': PanelLeftClose, 'panel-left-open': PanelLeftOpen,
  'panel-single': Square, columns: Columns2, 'pane-open-other': FolderOutput,
  'pane-swap': ArrowLeftRight, 'pane-move': ArrowRight, command: Command,
  bot: Bot, sparkles: Sparkles, send: Send, play: Play,
  'message-square': MessageSquare, 'alert-triangle': TriangleAlert,
  'alert-circle': CircleAlert, info: Info, 'check-circle': CheckCircle,
  search: Search, eye: Eye, sun: Sun, moon: Moon, megaphone: Megaphone,
  'external-link': ExternalLink, loader: LoaderCircle, stop: Square,
}

const icon = computed(() => libraryIcons[props.name])
const paths = computed<readonly string[]>(() => ICONS[props.name])
const dimension = computed(() => typeof props.size === 'number' ? `${props.size}px` : props.size)
</script>

<template>
  <component
    :is="icon"
    v-if="icon"
    class="icon"
    :class="{ 'icon-flipped': name === 'pane-open-other' }"
    :width="dimension"
    :height="dimension"
    :stroke-width="strokeWidth"
    aria-hidden="true"
    focusable="false"
  />
  <svg
    v-else
    class="icon"
    :class="{ 'icon-flipped': name === 'pane-open-other' }"
    :style="{ width: dimension, height: dimension }"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    :stroke-width="strokeWidth"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    focusable="false"
  >
    <path v-for="(d, i) in paths" :key="i" :d="d" />
  </svg>
</template>

<style scoped>
.icon {
  display: inline-block;
  flex: none;
  vertical-align: -0.15em;
}

.icon-flipped {
  transform: scaleX(-1);
}
</style>
