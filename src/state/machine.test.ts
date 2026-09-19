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
  { type: 'DOCKER_FOUND' },
  { type: 'DAEMON_UP' },
  { type: 'KEY_SUBMIT' },
  { type: 'KEY_VALID' },
  { type: 'MODEL_CHOSEN' },
  { type: 'REGISTRY_CHOSEN' },
  { type: 'PULL_DONE' },
  { type: 'CONFIG_WRITTEN' },
  { type: 'START_OK' },
  { type: 'ENTER_PANEL' },
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
  { name: 'ChooseModel' },
  { name: 'SelectRegistry' },
  { name: 'Pulling' },
  { name: 'WriteConfig' },
  { name: 'Starting' },
  { name: 'Done' },
  { name: 'Ready' },
  { name: 'Stopped' },
  { name: 'Upgrading' },
  { name: 'Error', code: 'E_PULL_FAILED', from: 'Pulling' },
]

const ALL_EVENTS: Event[] = [
  { type: 'BOOT' },
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
  { type: 'MODEL_CHOSEN' },
  { type: 'REGISTRY_CHOSEN' },
  { type: 'PULL_DONE' },
  { type: 'PULL_FAILED' },
  { type: 'CONFIG_WRITTEN' },
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
      'CheckDocker',
      'CheckDaemon',
      'NeedKey',
      'ValidateKey',
      'ChooseModel',
      'SelectRegistry',
      'Pulling',
      'WriteConfig',
      'Starting',
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
    const guide = run(INITIAL_STATE, [{ type: 'BOOT' }, { type: 'ACCEPT_TERMS' }, { type: 'DOCKER_MISSING' }])
    expect(guide.name).toBe('InstallDockerGuide')
    expect(transition(guide, { type: 'RECHECK_DOCKER' }).name).toBe('CheckDocker')
  })

  it('daemon 没起时进 StartDaemon，再检测回到 CheckDaemon', () => {
    const down = run(INITIAL_STATE, [
      { type: 'BOOT' },
      { type: 'ACCEPT_TERMS' },
      { type: 'DOCKER_FOUND' },
      { type: 'DAEMON_DOWN' },
    ])
    expect(down.name).toBe('StartDaemon')
    expect(transition(down, { type: 'TRY_START_DAEMON' }).name).toBe('CheckDaemon')
  })
})

describe('key 校验分支', () => {
  it('校验不过退回 NeedKey', () => {
    const validating = at(5)
    expect(validating.name).toBe('ValidateKey')
    expect(transition(validating, { type: 'KEY_INVALID' }).name).toBe('NeedKey')
  })
})

describe('错误与重试', () => {
  it('拉取失败进 Error(E_PULL_FAILED)，重试回到 Pulling', () => {
    const pulling = at(8)
    expect(pulling.name).toBe('Pulling')
    const err = transition(pulling, { type: 'PULL_FAILED', detail: '源不可达' })
    expect(err).toEqual({ name: 'Error', code: 'E_PULL_FAILED', from: 'Pulling', detail: '源不可达' })
    expect(transition(err, { type: 'RETRY' }).name).toBe('Pulling')
  })

  it('启动超时进 Error(E_START_TIMEOUT)，重试回到 Starting', () => {
    const starting = at(10)
    expect(starting.name).toBe('Starting')
    const err = transition(starting, { type: 'START_TIMEOUT', detail: 'opencode 未就绪' })
    expect(err.name).toBe('Error')
    expect(transition(err, { type: 'RETRY' }).name).toBe('Starting')
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
      const err: State = { name: 'Error', code, from: 'Pulling' }
      const next = transition(err, { type: 'RETRY' })
      expect(next.name).not.toBe('Error')
    }
  })

  it('Error 上的 BACK 回到来时的状态', () => {
    const err: State = { name: 'Error', code: 'E_UNKNOWN', from: 'ChooseModel' }
    expect(transition(err, { type: 'BACK' }).name).toBe('ChooseModel')
  })

  it('没有 detail 时不写 detail 字段', () => {
    const err = transition({ name: 'Pulling' }, { type: 'PULL_FAILED' })
    expect(err).toEqual({ name: 'Error', code: 'E_PULL_FAILED', from: 'Pulling' })
    expect('detail' in err).toBe(false)
  })
})

describe('运行期', () => {
  it('Ready 可以停止、重新启动', () => {
    const ready = at(12)
    expect(ready.name).toBe('Ready')
    const stopped = transition(ready, { type: 'STOP' })
    expect(stopped.name).toBe('Stopped')
    expect(transition(stopped, { type: 'START' }).name).toBe('Starting')
  })

  it('升级成功回 Ready，失败进 Error(E_UPDATE_FAILED)', () => {
    const up = transition(at(12), { type: 'UPGRADE' })
    expect(up.name).toBe('Upgrading')
    expect(transition(up, { type: 'UPGRADE_OK' }).name).toBe('Ready')
    const failed = transition(up, { type: 'UPGRADE_FAILED', detail: '健康检查未通过' })
    expect(failed).toMatchObject({ name: 'Error', code: 'E_UPDATE_FAILED', from: 'Upgrading' })
  })
})

describe('上一步', () => {
  it('输入 key 页能退回 Docker 检测', () => {
    expect(transition({ name: 'NeedKey' }, { type: 'BACK' }).name).toBe('CheckDaemon')
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
    const st: State = { name: 'Pulling' }
    const copy = { ...st }
    transition(st, { type: 'PULL_FAILED', detail: 'x' })
    expect(st).toEqual(copy)
  })
})

describe('步骤条', () => {
  it('六步，顺序与视觉稿一致', () => {
    expect(STEP_IDS).toEqual(['welcome', 'docker', 'key', 'model', 'pull', 'start'])
  })

  it('停在输入 key 时：前两步 done、当前步 current、其余 todo', () => {
    const steps = stepperOf({ name: 'NeedKey' })
    expect(steps.map((s) => s.status)).toEqual(['done', 'done', 'current', 'todo', 'todo', 'todo'])
  })

  it('同一屏里三种状态都会出现（视觉稿要求的三种样式）', () => {
    const statuses = new Set(stepperOf({ name: 'ChooseModel' }).map((s) => s.status))
    expect([...statuses].sort()).toEqual(['current', 'done', 'todo'])
  })

  it('出错时步骤条停在出错前的那一步', () => {
    expect(stepOf({ name: 'Error', code: 'E_PULL_FAILED', from: 'Pulling' })).toBe('pull')
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
      'ChooseModel',
      'SelectRegistry',
      'Pulling',
      'WriteConfig',
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
    expect(pageOf({ name: 'Pulling' })).toBe('pull')
    expect(pageOf({ name: 'Ready' })).toBe('dashboard')
  })

  it('Error 永远渲染错误页', () => {
    expect(pageOf({ name: 'Error', code: 'E_KEY_INVALID', from: 'NeedKey' })).toBe('error')
  })
})
