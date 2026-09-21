import { describe, expect, it } from 'vitest'
import {
  ERROR_CODES,
  INITIAL_STATE,
  STEP_IDS,
  type Event,
  type State,
  type StateName,
  isWizard,
  pageOf,
  run,
  stepOf,
  stepperOf,
  transition,
} from './machine'

/** 从 Idle 一路走到指定状态的事件序列，测试里反复用。 */
const HAPPY_PATH: Event[] = [
  { type: 'BOOT' },
  { type: 'ACCEPT_TERMS' },
  // I5：欢迎 → 输入 key → 一次授权 → 选模型 → 自动安装 → 完成
  { type: 'KEY_SUBMIT' },
  { type: 'KEY_VALID' },
  { type: 'CONSENT_GIVEN' },
  { type: 'AUTO_DONE' },
  { type: 'ENTER_PANEL' },
]

/** 一路走到「检测 Docker」那一页。I5 之后它不在主路径上了 ——
 *  只有自动安装撞上 Docker 问题、用户从错误页点「重试」时才会到那一页。 */
const TO_DOCKER: Event[] = [
  { type: 'BOOT' },
  { type: 'ACCEPT_TERMS' },
  { type: 'KEY_SUBMIT' },
  { type: 'KEY_VALID' },
  { type: 'CONSENT_GIVEN' },
  { type: 'FAIL', code: 'E_DOCKER_MISSING' },
  { type: 'RETRY' },
]

function at(n: number): State {
  return run(INITIAL_STATE, HAPPY_PATH.slice(0, n))
}

const ALL_STATES: State[] = [
  { name: 'Idle' },
  { name: 'Welcome' },
  { name: 'CheckDocker' },
  { name: 'InstallDockerGuide' },
  { name: 'CheckDaemon' },
  { name: 'StartDaemon' },
  { name: 'NeedKey' },
  { name: 'ValidateKey' },
  { name: 'Consent' },
  { name: 'ChooseModel' },
  { name: 'AutoInstalling' },
  { name: 'Starting' },
  { name: 'Done' },
  { name: 'Ready' },
  { name: 'Stopped' },
  { name: 'Upgrading' },
  { name: 'Error', code: 'E_PULL_FAILED', from: 'AutoInstalling' },
]

const ALL_EVENTS: Event[] = [
  { type: 'BOOT' },
  { type: 'RESUME_READY' },
  { type: 'ACCEPT_TERMS' },
  { type: 'DOCKER_MISSING' },
  { type: 'DOCKER_FOUND' },
  { type: 'RECHECK_DOCKER' },
  { type: 'DAEMON_DOWN' },
  { type: 'DAEMON_UP' },
  { type: 'TRY_START_DAEMON' },
  { type: 'KEY_SUBMIT' },
  { type: 'KEY_VALID' },
  { type: 'KEY_INVALID' },
  { type: 'CONSENT_GIVEN' },
  { type: 'CHOOSE_MODEL' },
  { type: 'MODEL_CHOSEN' },
  { type: 'AUTO_DONE' },
  { type: 'AUTO_FAILED', code: 'E_PORT_CONFLICT' },
  { type: 'START_OK' },
  { type: 'START_TIMEOUT' },
  { type: 'ENTER_PANEL' },
  { type: 'STOP' },
  { type: 'START' },
  { type: 'UPGRADE' },
  { type: 'UPGRADE_OK' },
  { type: 'UPGRADE_FAILED' },
  { type: 'BACK' },
  { type: 'RETRY' },
  { type: 'FAIL', code: 'E_UNKNOWN' },
]

describe('主路径', () => {
  it('从 Idle 一路走到 Ready', () => {
    const names = HAPPY_PATH.map((_, i) => at(i + 1).name)
    expect(names).toEqual([
      'Welcome',
      'NeedKey',
      'ValidateKey',
      'Consent',
      // 授权完**直接开装** —— 中间不再插一页「选模型」让用户点下一步
      'AutoInstalling',
      'Done',
      'Ready',
    ])
  })

  it('初始状态是 Idle', () => {
    expect(INITIAL_STATE).toEqual({ name: 'Idle' })
  })
})

describe('Docker 分支', () => {
  it('缺 Docker 时进安装引导，重新检测回到 CheckDocker', () => {
    const guide = run(INITIAL_STATE, [...TO_DOCKER, { type: 'DOCKER_MISSING' }])
    expect(guide.name).toBe('InstallDockerGuide')
    expect(transition(guide, { type: 'RECHECK_DOCKER' }).name).toBe('CheckDocker')
  })

  it('daemon 没起时进 StartDaemon，再检测回到 CheckDaemon', () => {
    const down = run(INITIAL_STATE, [...TO_DOCKER, { type: 'DOCKER_FOUND' }, { type: 'DAEMON_DOWN' }])
    expect(down.name).toBe('StartDaemon')
    expect(transition(down, { type: 'TRY_START_DAEMON' }).name).toBe('CheckDaemon')
  })

  it('I5：Docker 这一关过了就回到自动安装，不把用户送回选模型', () => {
    const daemon = run(INITIAL_STATE, [...TO_DOCKER, { type: 'DOCKER_FOUND' }])
    expect(daemon.name).toBe('CheckDaemon')
    expect(transition(daemon, { type: 'DAEMON_UP' }).name).toBe('AutoInstalling')
  })
})

describe('I5 · 一次授权与自动安装', () => {
  it('key 验过之后先做一次授权', () => {
    const validating = run(INITIAL_STATE, [
      { type: 'BOOT' },
      { type: 'ACCEPT_TERMS' },
      { type: 'KEY_SUBMIT' },
    ])
    const consent = transition(validating, { type: 'KEY_VALID' })
    expect(consent.name).toBe('Consent')
  })

  /** 本轮的硬指标：**授权之后一次都不用点**。中间插一页「选模型 → 下一步」就不算数。 */
  it('授权完直接进自动安装，中间没有任何一次点击', () => {
    expect(transition({ name: 'Consent' }, { type: 'CONSENT_GIVEN' }).name).toBe('AutoInstalling')
  })

  it('要用自带模型 key 的人走支线：授权页 → 选模型 → 自动安装', () => {
    const model = transition({ name: 'Consent' }, { type: 'CHOOSE_MODEL' })
    expect(model.name).toBe('ChooseModel')
    expect(transition(model, { type: 'MODEL_CHOSEN' }).name).toBe('AutoInstalling')
  })

  it('跳过选模型之后，步骤条上那一步显示成「已完成」', () => {
    const steps = stepperOf({ name: 'AutoInstalling' })
    expect(steps.map((x) => `${x.id}:${x.status}`)).toEqual([
      'welcome:done',
      'key:done',
      'consent:done',
      'model:done',
      'install:current',
    ])
  })

  it('自动安装成功进完成页，失败带着真实错误码进错误页', () => {
    const auto: State = { name: 'AutoInstalling' }
    expect(transition(auto, { type: 'AUTO_DONE' }).name).toBe('Done')
    const err = transition(auto, {
      type: 'AUTO_FAILED',
      code: 'E_PORT_CONFLICT',
      detail: 'Docker 拒绝发布端口 8100',
    })
    expect(err).toEqual({
      name: 'Error',
      code: 'E_PORT_CONFLICT',
      from: 'AutoInstalling',
      detail: 'Docker 拒绝发布端口 8100',
    })
  })

  it('E_PORT_CONFLICT 是独立错误码，重试回到自动安装', () => {
    expect(ERROR_CODES).toContain('E_PORT_CONFLICT')
    const err: State = { name: 'Error', code: 'E_PORT_CONFLICT', from: 'AutoInstalling' }
    expect(transition(err, { type: 'RETRY' }).name).toBe('AutoInstalling')
  })

  it('自动安装跑起来之后没有「上一步」', () => {
    expect(transition({ name: 'AutoInstalling' }, { type: 'BACK' }).name).toBe('AutoInstalling')
  })
})

describe('key 校验分支', () => {
  it('校验不过退回 NeedKey', () => {
    const validating = at(3)
    expect(validating.name).toBe('ValidateKey')
    expect(transition(validating, { type: 'KEY_INVALID' }).name).toBe('NeedKey')
  })
})

describe('错误与重试', () => {
  it('拉取失败进 Error(E_PULL_FAILED)，重试回到自动安装', () => {
    const auto = at(5)
    expect(auto.name).toBe('AutoInstalling')
    const err = transition(auto, { type: 'AUTO_FAILED', code: 'E_PULL_FAILED', detail: '源不可达' })
    expect(err).toEqual({
      name: 'Error',
      code: 'E_PULL_FAILED',
      from: 'AutoInstalling',
      detail: '源不可达',
    })
    expect(transition(err, { type: 'RETRY' }).name).toBe('AutoInstalling')
  })

  it('运行面板上的「启动」超时仍然回到启动页', () => {
    const starting = run(INITIAL_STATE, [{ type: 'RESUME_READY' }, { type: 'STOP' }, { type: 'START' }])
    expect(starting.name).toBe('Starting')
    const err = transition(starting, { type: 'START_TIMEOUT', detail: 'opencode 未就绪' })
    expect(err.name).toBe('Error')
    expect(transition(err, { type: 'RETRY' }).name).toBe('AutoInstalling')
  })

  it('FAIL 在任何状态下都能进 Error，并记住来时的状态', () => {
    for (const st of ALL_STATES) {
      const err = transition(st, { type: 'FAIL', code: 'E_UNKNOWN', detail: 'x' })
      expect(err.name).toBe('Error')
      if (err.name === 'Error') {
        expect(err.from).toBe(st.name === 'Error' ? st.from : st.name)
      }
    }
  })

  it('每个错误码都有确定的重试去向，且不会停在 Error 上', () => {
    for (const code of ERROR_CODES) {
      const err: State = { name: 'Error', code, from: 'AutoInstalling' }
      const next = transition(err, { type: 'RETRY' })
      expect(next.name).not.toBe('Error')
    }
  })

  it('Error 上的 BACK 回到来时的状态', () => {
    const err: State = { name: 'Error', code: 'E_UNKNOWN', from: 'ChooseModel' }
    expect(transition(err, { type: 'BACK' }).name).toBe('ChooseModel')
  })

  it('没有 detail 时不写 detail 字段', () => {
    const err = transition({ name: 'AutoInstalling' }, { type: 'AUTO_FAILED', code: 'E_PULL_FAILED' })
    expect(err).toEqual({ name: 'Error', code: 'E_PULL_FAILED', from: 'AutoInstalling' })
    expect('detail' in err).toBe(false)
  })
})

describe('运行期', () => {
  it('Ready 可以停止、重新启动', () => {
    const ready = at(7)
    expect(ready.name).toBe('Ready')
    const stopped = transition(ready, { type: 'STOP' })
    expect(stopped.name).toBe('Stopped')
    expect(transition(stopped, { type: 'START' }).name).toBe('Starting')
  })

  it('升级成功回 Ready，失败进 Error(E_UPDATE_FAILED)', () => {
    const up = transition(at(7), { type: 'UPGRADE' })
    expect(up.name).toBe('Upgrading')
    expect(transition(up, { type: 'UPGRADE_OK' }).name).toBe('Ready')
    const failed = transition(up, { type: 'UPGRADE_FAILED', detail: '健康检查未通过' })
    expect(failed).toMatchObject({ name: 'Error', code: 'E_UPDATE_FAILED', from: 'Upgrading' })
  })
})

describe('上一步（I5 调整顺序之后）', () => {
  it('输入 key 页退回欢迎页 —— 它现在是第二步', () => {
    expect(transition({ name: 'NeedKey' }, { type: 'BACK' }).name).toBe('Welcome')
  })

  it('一次授权页退回输入 key', () => {
    expect(transition({ name: 'Consent' }, { type: 'BACK' }).name).toBe('NeedKey')
  })

  it('Docker 检测页退回一次授权 —— 它现在归在「自动安装」这一步里', () => {
    expect(transition({ name: 'CheckDocker' }, { type: 'BACK' }).name).toBe('Consent')
    expect(transition({ name: 'CheckDaemon' }, { type: 'BACK' }).name).toBe('Consent')
    expect(transition({ name: 'InstallDockerGuide' }, { type: 'BACK' }).name).toBe('Consent')
  })

  it('选模型页退回一次授权', () => {
    expect(transition({ name: 'ChooseModel' }, { type: 'BACK' }).name).toBe('Consent')
  })

  it('每一个向导状态的「上一步」都必须落在它前面（不许往回跳到后面的步骤）', () => {
    const order = ['welcome', 'key', 'consent', 'model', 'install']
    const wizard: StateName[] = [
      'NeedKey',
      'ValidateKey',
      'Consent',
      'CheckDocker',
      'InstallDockerGuide',
      'CheckDaemon',
      'StartDaemon',
      'ChooseModel',
      'AutoInstalling',
    ]
    for (const name of wizard) {
      const from = { name } as State
      const back = transition(from, { type: 'BACK' })
      if (back.name === name) continue // 没有上一步的跳过
      const a = order.indexOf(stepOf(from)!)
      const b = order.indexOf(stepOf(back)!)
      expect(b, `${name} 的上一步是 ${back.name}`).toBeLessThanOrEqual(a)
    }
  })

  it('欢迎页没有上一步', () => {
    expect(transition({ name: 'Welcome' }, { type: 'BACK' }).name).toBe('Welcome')
  })

  it('运行面板没有上一步', () => {
    expect(transition({ name: 'Ready' }, { type: 'BACK' }).name).toBe('Ready')
  })
})

describe('健壮性', () => {
  it('任何状态收到不该收的事件都原样返回，不抛异常', () => {
    for (const st of ALL_STATES) {
      for (const ev of ALL_EVENTS) {
        expect(() => transition(st, ev)).not.toThrow()
      }
    }
  })

  it('转换结果永远是合法状态', () => {
    const legal = new Set<StateName>(ALL_STATES.map((s) => s.name))
    for (const st of ALL_STATES) {
      for (const ev of ALL_EVENTS) {
        expect(legal.has(transition(st, ev).name)).toBe(true)
      }
    }
  })

  it('transition 不改原状态对象（纯函数）', () => {
    const st: State = { name: 'AutoInstalling' }
    const copy = { ...st }
    transition(st, { type: 'AUTO_FAILED', code: 'E_PULL_FAILED', detail: 'x' })
    expect(st).toEqual(copy)
  })
})

describe('步骤条', () => {
  it('五步，顺序是 I5 的向导顺序', () => {
    expect(STEP_IDS).toEqual(['welcome', 'key', 'consent', 'model', 'install'])
  })

  it('停在输入 key 时：欢迎 done、key current、其余 todo', () => {
    const steps = stepperOf({ name: 'NeedKey' })
    expect(steps.map((s) => s.status)).toEqual(['done', 'current', 'todo', 'todo', 'todo'])
  })

  it('停在一次授权时前两步都是 done', () => {
    const steps = stepperOf({ name: 'Consent' })
    expect(steps.map((s) => s.status)).toEqual(['done', 'done', 'current', 'todo', 'todo'])
  })

  it('Docker 的几个状态都归在「自动安装」这一步', () => {
    for (const name of ['CheckDocker', 'InstallDockerGuide', 'CheckDaemon', 'StartDaemon'] as const) {
      expect(stepOf({ name })).toBe('install')
    }
  })

  it('同一屏里三种状态都会出现（视觉稿要求的三种样式）', () => {
    const statuses = new Set(stepperOf({ name: 'ChooseModel' }).map((s) => s.status))
    expect([...statuses].sort()).toEqual(['current', 'done', 'todo'])
  })

  it('出错时步骤条停在出错前的那一步', () => {
    expect(stepOf({ name: 'Error', code: 'E_PULL_FAILED', from: 'AutoInstalling' })).toBe('install')
  })

  it('运行面板不在步骤条里', () => {
    expect(stepOf({ name: 'Ready' })).toBeNull()
    expect(isWizard({ name: 'Ready' })).toBe(false)
    expect(isWizard({ name: 'NeedKey' })).toBe(true)
    expect(isWizard({ name: 'Error', code: 'E_UNKNOWN', from: 'NeedKey' })).toBe(false)
  })

  it('每个向导状态都能映射到某一步', () => {
    const wizardStates: StateName[] = [
      'Welcome',
      'CheckDocker',
      'InstallDockerGuide',
      'CheckDaemon',
      'StartDaemon',
      'NeedKey',
      'ValidateKey',
      'Consent',
      'ChooseModel',
      'AutoInstalling',
      'Starting',
      'Done',
    ]
    for (const name of wizardStates) {
      expect(stepOf({ name } as State)).not.toBeNull()
    }
  })
})

describe('页面映射', () => {
  it('每个状态都能映射到一个页面', () => {
    for (const st of ALL_STATES) {
      expect(pageOf(st)).toBeTruthy()
    }
  })

  it('三张视觉稿对应的页面', () => {
    expect(pageOf({ name: 'NeedKey' })).toBe('key')
    expect(pageOf({ name: 'Ready' })).toBe('dashboard')
  })

  it('I5 新增的两页', () => {
    expect(pageOf({ name: 'Consent' })).toBe('consent')
    expect(pageOf({ name: 'AutoInstalling' })).toBe('auto')
  })

  it('Error 永远渲染错误页', () => {
    expect(pageOf({ name: 'Error', code: 'E_KEY_INVALID', from: 'NeedKey' })).toBe('error')
  })
})

describe('第二次打开启动器（M2 新增）', () => {
  it('RESUME_READY 从 Idle 直接进运行面板', () => {
    expect(transition(INITIAL_STATE, { type: 'RESUME_READY' })).toEqual({ name: 'Ready' })
  })

  it('boot_state 是异步回来的，那时候通常已经在 Welcome 了，也要能跳', () => {
    const welcome = transition(INITIAL_STATE, { type: 'BOOT' })
    expect(welcome.name).toBe('Welcome')
    expect(transition(welcome, { type: 'RESUME_READY' })).toEqual({ name: 'Ready' })
  })

  it('已经走进向导之后不再理会这个事件（免得把用户从半路拽走）', () => {
    const inWizard = run(INITIAL_STATE, [{ type: 'BOOT' }, { type: 'ACCEPT_TERMS' }])
    expect(inWizard.name).toBe('NeedKey')
    expect(transition(inWizard, { type: 'RESUME_READY' })).toEqual(inWizard)
  })
})

describe('M2 新增的两个错误码', () => {
  it('都在 ERROR_CODES 里且有确定的重试去向', () => {
    for (const code of ['E_COMPOSE_FETCH', 'E_CONFIG_WRITE'] as const) {
      expect(ERROR_CODES).toContain(code)
      const err = transition({ name: 'AutoInstalling' }, { type: 'FAIL', code })
      expect(err).toMatchObject({ name: 'Error', code, from: 'AutoInstalling' })
      // 重试回到自动安装那一页从头跑一遍：配置与 compose 文件都是在那一步写的
      expect(transition(err, { type: 'RETRY' })).toEqual({ name: 'AutoInstalling' })
    }
  })
})

describe('Docker 页一次点击就该走到下一页（M2 修的 UX 问题）', () => {
  it('CheckDocker 连发两个事件后回到自动安装（I5：Docker 好了就接着自动装）', () => {
    const start = run(INITIAL_STATE, TO_DOCKER)
    expect(start.name).toBe('CheckDocker')
    // 页面在「装了 + daemon 在跑 + 版本够」时一次把两个事件都发出去
    const after = run(start, [{ type: 'DOCKER_FOUND' }, { type: 'DAEMON_UP' }])
    expect(after.name).toBe('AutoInstalling')
  })

  it('CheckDocker 与 CheckDaemon 渲染的确实是同一页（所以只发一个事件会看起来没反应）', () => {
    expect(pageOf({ name: 'CheckDocker' })).toBe('docker')
    expect(pageOf({ name: 'CheckDaemon' })).toBe('docker')
  })
})
