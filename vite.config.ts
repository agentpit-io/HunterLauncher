import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// 总控规则红线 1：演示数据只允许出现在 VITE_DEMO=1 的开发构建里。
// 发布构建由 CI 设置 HUNTER_CHANNEL=release，此时若还带着 VITE_DEMO 就直接让构建失败，
// 保证「发布包里不可能进入演示模式」这条约束是被机器而不是被人记住的。
if (process.env.VITE_DEMO === '1' && process.env.HUNTER_CHANNEL === 'release') {
  throw new Error('拒绝构建：HUNTER_CHANNEL=release 时不允许 VITE_DEMO=1（总控规则红线 1）')
}

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Tauri 期望固定端口，且不要在端口被占用时自动换端口
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ['**/src-tauri/**'] } },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    // Tauri v2 在 Windows 上用 Chromium（Edge WebView2），macOS/Linux 上用 WebKit
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari15',
    minify: process.env.TAURI_ENV_DEBUG ? false : 'esbuild',
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    chunkSizeWarningLimit: 1200,
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
    reporters: ['default'],
  },
})
