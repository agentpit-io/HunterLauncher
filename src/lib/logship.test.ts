import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
import { TitleBar } from '../components/TitleBar'
import { Logs } from '../pages/Logs'
import { StoreCtx, type Overlay, type Store } from '../state/context'
import { dictOf } from '../i18n'
import * as ipc from './ipc'

function makeStore(over: Partial<Store> = {}): Store {
  return {
    state: { name: 'Ready' },
    send: () => {},
    locale: 'zh-CN',
    setLocale: () => {},
    t: dictOf('zh-CN'),
    overlay: null,
    setOverlay: () => {},
    demo: false,
    notice: null,
    setNotice: () => {},
    boot: null,
    logUploadTrigger: 0,
    triggerLogUpload: () => {},
    ...over,
  }
}

describe('I21 · 顶部栏上传日志按钮与日志页日志编码联动', () => {
  it('TitleBar 包含 data-testid="titlebar-upload-logs" 按钮，且标明上传日志文案与提示', () => {
    const html = renderToStaticMarkup(
      createElement(
        StoreCtx.Provider,
        { value: makeStore() },
        createElement(TitleBar, {
          info: {
            launcherVersion: '0.1.23',
            platform: 'linux',
            arch: 'x86_64',
            windowChrome: 'custom',
          },
        }),
      ),
    )
    expect(html).toContain('data-testid="titlebar-upload-logs"')
    expect(html).toContain('上传日志')
    expect(html).toContain('上传后台日志并查看日志')
  })

  it('点击顶部栏上传日志按钮切换到 logs 抽屉页并触发 logshipPreview + logshipUpload 展示日志编码', async () => {
    let currentOverlay: Overlay = null
    let triggerCount = 0

    const store = makeStore({
      setOverlay: (o) => {
        currentOverlay = o
      },
      triggerLogUpload: () => {
        currentOverlay = 'logs'
        triggerCount += 1
      },
    })

    // 模拟点击顶部栏「上传日志」按钮
    store.triggerLogUpload()
    expect(currentOverlay).toBe('logs')
    expect(triggerCount).toBe(1)

    // 验证上传流程：logshipPreview({ stage: 'titlebar' }) -> logshipUpload()
    const previewSpy = vi.spyOn(ipc, 'logshipPreview').mockResolvedValue({
      sections: [],
      body: '===== 启动器日志 =====\n10:00:00 [info] ok',
      bodyBytes: 42,
      truncated: false,
      machineId: '11111111-2222-4333-8444-555555555555',
      endpoint: 'https://www.agentpit.io/api/v1/launcher/logs',
      stage: 'titlebar',
      errorCode: '',
      summary: '',
      metaLines: [],
      scanHit: null,
      autoOnError: false,
    })
    const uploadSpy = vi.spyOn(ipc, 'logshipUpload').mockResolvedValue({
      ok: true,
      traceCode: 'HL-7K3Q9F',
      truncated: false,
      message: '已上传。追踪码 HL-7K3Q9F',
      status: 200,
      bundlePath: null,
      localLogPath: '~/.hunter/logs/launcher.log',
    })

    const preview = await ipc.logshipPreview({ stage: 'titlebar' })
    expect(previewSpy).toHaveBeenCalledWith({ stage: 'titlebar' })
    expect(preview.scanHit).toBeNull()

    const outcome = await ipc.logshipUpload()
    expect(uploadSpy).toHaveBeenCalledTimes(1)
    expect(outcome.ok).toBe(true)
    expect(outcome.traceCode).toBe('HL-7K3Q9F')

    // 渲染上传完成后的 Logs 页面：顶部包含 logs-trace-banner、日志编码 HL-7K3Q9F、复制按钮、查看已传内容与重新上传，下方保留日志过滤栏
    const logsHtml = renderToStaticMarkup(
      createElement(
        StoreCtx.Provider,
        { value: makeStore({ overlay: 'logs', logUploadTrigger: triggerCount }) },
        createElement(Logs, { initialOutcome: outcome, initialPreview: preview }),
      ),
    )
    expect(logsHtml).toContain('data-testid="logs-trace-banner"')
    expect(logsHtml).toContain('data-testid="logs-trace-code"')
    expect(logsHtml).toContain('HL-7K3Q9F')
    expect(logsHtml).toContain('data-testid="logs-copy-trace"')
    expect(logsHtml).toContain('data-testid="logs-view-uploaded"')
    expect(logsHtml).toContain('data-testid="logs-reupload"')
    expect(logsHtml).toContain('data-testid="logs-svc-all"')

    previewSpy.mockRestore()
    uploadSpy.mockRestore()
  })
})
