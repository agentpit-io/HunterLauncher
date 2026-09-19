/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** 演示数据模式开关。只在开发构建里被设为 "1"，发布构建永远为空（总控规则红线 1）。 */
  readonly VITE_DEMO?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
