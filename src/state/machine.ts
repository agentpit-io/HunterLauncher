/*
 * 启动器状态机（技术方案第 4 节）
 * ---------------------------------------------------------------------------
 * 这里是纯函数，不碰 React、不碰 Tauri、不做副作用，便于用 vitest 全量覆盖。
 * 页面由状态驱动：pageOf(state) 决定渲染哪一页，stepperOf(state) 决定左侧步骤条的样子。
 *
 * 与方案第 4 节的三处增补（前两处已在 M1 成果文档里写明，第三处是 I4）：
 *   1. 方案从 Idle 直接进 CheckDocker，这里在前面补了 Welcome（视觉稿的步骤条第一项是「欢迎」）。
 *   2. 方案里 Starting 成功后直接进 Ready，这里中间补了 Done（完成页），
 *      因为「打开浏览器」应该是用户点的，不是启动器替他点的。
 *   3. **I4 把「输入 key」挪到了「检测 Docker」前面**：
 *        欢迎 → 输入 key → Docker → 选模型 → 拉取镜像 → 启动
 *      理由是 I4 新加的 AI 诊断助手要调 Hunter 网关，而网关的凭据就是那把 key；
 *      Docker 检测又恰恰是最容易出事、最需要 AI 帮忙的一步。
 *      填 key 这一步只需要联网，本来就不依赖 Docker，挪到前面没有任何代价。
 */

export type StateName =
  | 'Idle'
  | 'Welcome'
  | 'CheckDocker'
  | 'InstallDockerGuide'
  | 'CheckDaemon'
  | 'StartDaemon'
  | 'NeedKey'
  | 'ValidateKey'
  | 'ChooseModel'
  | 'SelectRegistry'
  | 'Pulling'
  | 'WriteConfig'
  | 'Starting'
  | 'Done'
  | 'Ready'
  | 'Stopped'
  | 'Upgrading'
  | 'Error'

/** 技术方案第 18 节的错误码表，外加两个本地码。 */
export type ErrorCode =
  | 'E_DOCKER_MISSING'
  | 'E_DAEMON_DOWN'
  | 'E_WSL_MISSING'
  | 'E_KEY_INVALID'
  | 'E_QUOTA_EXHAUSTED'
  | 'E_PULL_FAILED'
  | 'E_PORT_IN_USE'
  | 'E_START_TIMEOUT'
  | 'E_PROXY_BLOCK'
  | 'E_UPDATE_FAILED'
  // 下面三个方案 §18 没有，是实现时按真实失败模式补的：
  // compose 文件取不到（方案假设从 Release 资产下载，实测 Release 没有资产）、写配置失败、
  // 以及 I1 补的「compose 项目名被另一个工作目录占着」（待办池 P0-5）
  | 'E_COMPOSE_FETCH'
  | 'E_CONFIG_WRITE'
  | 'E_PROJECT_CONFLICT'
  // I2 补的：网关限流（HTTP 429）。**key 是好的**，等一分钟就行 ——
  // 实测限流只作用于 POST /v1/chat/completions，启动器用的 /quota 与 /v1/models
  // 都不受限，所以这一条平时打不到；留着是为了别把「等一下」显示成「key 无效」
  | 'E_RATE_LIMITED'
  | 'E_NOT_IMPLEMENTED'
  | 'E_UNKNOWN'

export const ERROR_CODES: ErrorCode[] = [
  'E_DOCKER_MISSING',
  'E_DAEMON_DOWN',
  'E_WSL_MISSING',
  'E_KEY_INVALID',
  'E_QUOTA_EXHAUSTED',
  'E_PULL_FAILED',
  'E_PORT_IN_USE',
  'E_START_TIMEOUT',
  'E_PROXY_BLOCK',
  'E_UPDATE_FAILED',
  'E_COMPOSE_FETCH',
  'E_CONFIG_WRITE',
  'E_PROJECT_CONFLICT',
  'E_RATE_LIMITED',
  'E_NOT_IMPLEMENTED',
  'E_UNKNOWN',
]

export type State =
  | { name: Exclude<StateName, 'Error'> }
  | { name: 'Error'; code: ErrorCode; from: Exclude<StateName, 'Error'>; detail?: string }

export type Event =
  | { type: 'BOOT' }
  /** 已经装过：直接进运行面板，不重走向导（第二次打开启动器） */
  | { type: 'RESUME_READY' }
  | { type: 'ACCEPT_TERMS' }
  | { type: 'DOCKER_MISSING' }
  | { type: 'DOCKER_FOUND' }
  | { type: 'RECHECK_DOCKER' }
  | { type: 'DAEMON_DOWN' }
  | { type: 'DAEMON_UP' }
  | { type: 'TRY_START_DAEMON' }
  | { type: 'KEY_SUBMIT' }
  | { type: 'KEY_VALID' }
  | { type: 'KEY_INVALID' }
  | { type: 'MODEL_CHOSEN' }
  | { type: 'REGISTRY_CHOSEN' }
  | { type: 'PULL_DONE' }
  | { type: 'PULL_FAILED'; detail?: string }
  | { type: 'CONFIG_WRITTEN' }
  | { type: 'START_OK' }
  | { type: 'START_TIMEOUT'; detail?: string }
  | { type: 'ENTER_PANEL' }
  | { type: 'STOP' }
  | { type: 'START' }
  | { type: 'UPGRADE' }
  | { type: 'UPGRADE_OK' }
  | { type: 'UPGRADE_FAILED'; detail?: string }
  | { type: 'BACK' }
  | { type: 'RETRY' }
  | { type: 'FAIL'; code: ErrorCode; detail?: string }

export const INITIAL_STATE: State = { name: 'Idle' }

/** 向导里「上一步」能退到哪。没写在这里的状态没有上一步。 */
const BACK_TARGET: Partial<Record<StateName, Exclude<StateName, 'Error'>>> = {
  NeedKey: 'Welcome',
  ValidateKey: 'NeedKey',
  CheckDocker: 'NeedKey',
  InstallDockerGuide: 'NeedKey',
  CheckDaemon: 'NeedKey',
  StartDaemon: 'CheckDaemon',
  ChooseModel: 'CheckDaemon',
  SelectRegistry: 'ChooseModel',
  Pulling: 'ChooseModel',
}

/** 出错后「重试」回到哪个状态。默认回到出错前的那个状态。 */
const RETRY_TARGET: Partial<Record<ErrorCode, Exclude<StateName, 'Error'>>> = {
  E_DOCKER_MISSING: 'CheckDocker',
  E_DAEMON_DOWN: 'CheckDaemon',
  E_WSL_MISSING: 'CheckDocker',
  E_KEY_INVALID: 'NeedKey',
  E_QUOTA_EXHAUSTED: 'ChooseModel',
  // 限流是等一分钟就好的事，重试当然回到填 key 那一页
  E_RATE_LIMITED: 'NeedKey',
  E_PULL_FAILED: 'Pulling',
  E_START_TIMEOUT: 'Starting',
  E_COMPOSE_FETCH: 'Pulling',
  E_CONFIG_WRITE: 'Pulling',
}

function s(name: Exclude<StateName, 'Error'>): State {
  return { name }
}

function fail(from: State, code: ErrorCode, detail?: string): State {
  const origin = from.name === 'Error' ? from.from : from.name
  return detail === undefined ? { name: 'Error', code, from: origin } : { name: 'Error', code, from: origin, detail }
}

/**
 * 状态机的唯一转换函数。
 * 收到当前状态下不该出现的事件时**原样返回**，不抛异常也不乱跳 ——
 * 界面上一个慢回来的异步结果不应该把用户从别的页面拽走。
 */
export function transition(state: State, event: Event): State {
  // FAIL 在任何状态下都能把机器推进 Error
  if (event.type === 'FAIL') return fail(state, event.code, event.detail)

  if (state.name === 'Error') {
    if (event.type === 'RETRY') return s(RETRY_TARGET[state.code] ?? state.from)
    if (event.type === 'BACK') return s(state.from)
    return state
  }

  if (event.type === 'BACK') {
    const target = BACK_TARGET[state.name]
    return target ? s(target) : state
  }

  switch (state.name) {
    case 'Idle':
      if (event.type === 'BOOT') return s('Welcome')
      if (event.type === 'RESUME_READY') return s('Ready')
      return state

    case 'Welcome':
      // I4：欢迎之后先填 key（AI 诊断助手要用它调网关），再检测 Docker
      if (event.type === 'ACCEPT_TERMS') return s('NeedKey')
      // boot_state 是异步回来的，那时候通常已经在 Welcome 了，所以这一步也要能跳
      if (event.type === 'RESUME_READY') return s('Ready')
      return state

    case 'CheckDocker':
      if (event.type === 'DOCKER_MISSING') return s('InstallDockerGuide')
      if (event.type === 'DOCKER_FOUND') return s('CheckDaemon')
      return state

    case 'InstallDockerGuide':
      return event.type === 'RECHECK_DOCKER' ? s('CheckDocker') : state

    case 'CheckDaemon':
      if (event.type === 'DAEMON_DOWN') return s('StartDaemon')
      if (event.type === 'DAEMON_UP') return s('ChooseModel')
      return state

    case 'StartDaemon':
      if (event.type === 'TRY_START_DAEMON' || event.type === 'RECHECK_DOCKER') return s('CheckDaemon')
      return state

    case 'NeedKey':
      return event.type === 'KEY_SUBMIT' ? s('ValidateKey') : state

    case 'ValidateKey':
      if (event.type === 'KEY_VALID') return s('CheckDocker')
      if (event.type === 'KEY_INVALID') return s('NeedKey')
      return state

    case 'ChooseModel':
      return event.type === 'MODEL_CHOSEN' ? s('SelectRegistry') : state

    case 'SelectRegistry':
      return event.type === 'REGISTRY_CHOSEN' ? s('Pulling') : state

    case 'Pulling':
      if (event.type === 'PULL_DONE') return s('WriteConfig')
      if (event.type === 'PULL_FAILED') return fail(state, 'E_PULL_FAILED', event.detail)
      return state

    case 'WriteConfig':
      return event.type === 'CONFIG_WRITTEN' ? s('Starting') : state

    case 'Starting':
      if (event.type === 'START_OK') return s('Done')
      if (event.type === 'START_TIMEOUT') return fail(state, 'E_START_TIMEOUT', event.detail)
      return state

    case 'Done':
      return event.type === 'ENTER_PANEL' ? s('Ready') : state

    case 'Ready':
      if (event.type === 'STOP') return s('Stopped')
      if (event.type === 'UPGRADE') return s('Upgrading')
      return state

    case 'Stopped':
      return event.type === 'START' ? s('Starting') : state

    case 'Upgrading':
      if (event.type === 'UPGRADE_OK') return s('Ready')
      if (event.type === 'UPGRADE_FAILED') return fail(state, 'E_UPDATE_FAILED', event.detail)
      return state

    default:
      // 穷尽性检查：新增状态忘了在上面处理时，这一行会编译不过
      return assertNever(state)
  }
}

function assertNever(x: never): never {
  throw new Error(`状态机遇到未处理的状态：${JSON.stringify(x)}`)
}

/** 连续投递多个事件，方便写测试与「一路跑到某个状态」。 */
export function run(state: State, events: Event[]): State {
  return events.reduce(transition, state)
}

// ── 左侧步骤条 ────────────────────────────────────────────────────────────

export type StepId = 'welcome' | 'key' | 'docker' | 'model' | 'pull' | 'start'
/** 步骤条的顺序。I4 把 key 挪到了 docker 前面，见文件头的说明。 */
export const STEP_IDS: StepId[] = ['welcome', 'key', 'docker', 'model', 'pull', 'start']

const STATE_STEP: Partial<Record<StateName, StepId>> = {
  Welcome: 'welcome',
  NeedKey: 'key',
  ValidateKey: 'key',
  CheckDocker: 'docker',
  InstallDockerGuide: 'docker',
  CheckDaemon: 'docker',
  StartDaemon: 'docker',
  ChooseModel: 'model',
  SelectRegistry: 'pull',
  Pulling: 'pull',
  WriteConfig: 'start',
  Starting: 'start',
  Done: 'start',
}

/** 当前处在步骤条的哪一步；不在向导里（Ready/Stopped/…）返回 null。 */
export function stepOf(state: State): StepId | null {
  const name = state.name === 'Error' ? state.from : state.name
  return STATE_STEP[name] ?? null
}

export type StepStatus = 'done' | 'current' | 'todo'

/** 步骤条三种状态：已完成（灰蓝实心）、当前（金色实心）、未到（空心）。 */
export function stepperOf(state: State): { id: StepId; status: StepStatus }[] {
  const current = stepOf(state)
  const idx = current ? STEP_IDS.indexOf(current) : STEP_IDS.length
  return STEP_IDS.map((id, i) => ({
    id,
    status: i < idx ? 'done' : i === idx ? 'current' : 'todo',
  }))
}

// ── 页面路由 ──────────────────────────────────────────────────────────────

export type PageId =
  | 'welcome'
  | 'docker'
  | 'key'
  | 'model'
  | 'pull'
  | 'start'
  | 'done'
  | 'dashboard'
  | 'error'

const STATE_PAGE: Record<Exclude<StateName, 'Error'>, PageId> = {
  Idle: 'welcome',
  Welcome: 'welcome',
  CheckDocker: 'docker',
  InstallDockerGuide: 'docker',
  CheckDaemon: 'docker',
  StartDaemon: 'docker',
  NeedKey: 'key',
  ValidateKey: 'key',
  ChooseModel: 'model',
  SelectRegistry: 'pull',
  Pulling: 'pull',
  WriteConfig: 'start',
  Starting: 'start',
  Done: 'done',
  Ready: 'dashboard',
  Stopped: 'dashboard',
  Upgrading: 'dashboard',
}

export function pageOf(state: State): PageId {
  return state.name === 'Error' ? 'error' : STATE_PAGE[state.name]
}

/** 是否还在向导里（决定要不要渲染左侧步骤条与底部按钮条）。 */
export function isWizard(state: State): boolean {
  return stepOf(state) !== null && state.name !== 'Error'
}
