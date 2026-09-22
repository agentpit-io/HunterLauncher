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
 *      理由是 I4 新加的 AI 诊断助手要调 Hunter 网关，而网关的凭据就是那把 key；
 *      Docker 检测又恰恰是最容易出事、最需要 AI 帮忙的一步。
 *      填 key 这一步只需要联网，本来就不依赖 Docker，挪到前面没有任何代价。
 *   4. **I5 把向导改成「一次授权 + 全自动」**（AI 自动驾驶安装方案 §二、§七）：
 *        欢迎 → 输入 key → 一次授权 → 选模型 → 自动安装 → 完成
 *      - 新增 `Consent`（一次授权页）与 `AutoInstalling`（实时过程流）。
 *      - **删掉 `SelectRegistry` / `Pulling` / `WriteConfig`**：选源、写配置、拉镜像、
 *        起容器现在都由 Rust 侧的总指挥连着跑完，界面只负责把过程直播出来，
 *        中间不再有「用户点下一步」的断点（这正是这一轮要解决的问题）。
 *      - `Starting` 与「启动」页留着：运行面板上「停止 → 启动」走的是它，那不是安装。
 *      - Docker 相关的四个状态留着：自动安装撞上 Docker 问题、用户从错误页点「重试」时
 *        仍然回到那一页，它上面的安装指引是有用的。它们在步骤条上归到「自动安装」这一步。
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
  | 'Consent'
  | 'ChooseModel'
  | 'AutoInstalling'
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
  // I5：**起容器时** Docker 自己报的端口冲突（`port is already allocated`）。
  // 0.1.4 把它归成了 E_START_TIMEOUT，规则层于是认不出来 —— 用户 Mac 上那次失败的第 2 条根因
  | 'E_PORT_CONFLICT'
  // I6：本机的 docker 凭据助手（docker-credential-*）不在子进程看得到的 PATH 上。
  // 0.1.5 把它归成了 E_PULL_FAILED，规则层照着「拉不动 = 源不通」在两个镜像源之间
  // 来回换了三次 —— 这是**本机配置**的问题，换源永远修不好
  | 'E_CRED_HELPER'
  | 'E_START_TIMEOUT'
  // I9 补的：Hunter 自己那套内置运行时（~/.hunter/runtime）装着、虚拟机没起来。
  // 和 E_DAEMON_DOWN 分开，是因为两个现场的解法完全不同 —— 0.1.8 在用户 Mac 上
  // 把前者判成了后者，于是去「启动用户的 Colima」，而用户根本没装过 Colima
  | 'E_BUILTIN_DOWN'
  // I10 补的两个。0.1.9 在用户 Mac 上这两个现场都被报成了 E_START_TIMEOUT，
  // 规则层因此 unknown：启动器自带的虚拟机镜像里没有 systemd-resolved，
  // /etc/resolv.conf 出厂就是断链 —— 虚拟机和它里面所有容器都没有 DNS。
  // 「服务没就绪」和「根本没有 DNS」的修法完全不同，错误码必须分开
  | 'E_RUNTIME_NO_DNS'
  | 'E_CONTAINER_OFFLINE'
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
  'E_PORT_CONFLICT',
  'E_CRED_HELPER',
  'E_START_TIMEOUT',
  'E_BUILTIN_DOWN',
  'E_RUNTIME_NO_DNS',
  'E_CONTAINER_OFFLINE',
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
  /** 用户在一次授权页上做完了选择（三档都算）。**直接进自动安装** ——
   *  「授权之后不需要任何点击」是这一轮的硬指标，中间不能再插一页让他点「下一步」 */
  | { type: 'CONSENT_GIVEN' }
  /** 授权页上的「我要用自己的模型 key」。这是一条支线，主路径不经过它 */
  | { type: 'CHOOSE_MODEL' }
  | { type: 'MODEL_CHOSEN' }
  /** 自动安装跑完了，六个服务健康 */
  | { type: 'AUTO_DONE' }
  /** 自动安装修不好，带着真实错误码进最终错误页 */
  | { type: 'AUTO_FAILED'; code: ErrorCode; detail?: string }
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
  /**
   * 后端把 Docker 修好并已经自己把安装接着跑起来了（I9 的 P0-3）。
   *
   * 和 `RETRY` 的区别：`RETRY` 是用户点的，`RESUME_INSTALL` 是后端说的，
   * **从任何状态都能进**（包括错误页）—— 界面只是跟上后端已经发生的事实。
   */
  | { type: 'RESUME_INSTALL' }
  | { type: 'FAIL'; code: ErrorCode; detail?: string }

export const INITIAL_STATE: State = { name: 'Idle' }

/** 向导里「上一步」能退到哪。没写在这里的状态没有上一步。 */
const BACK_TARGET: Partial<Record<StateName, Exclude<StateName, 'Error'>>> = {
  NeedKey: 'Welcome',
  ValidateKey: 'NeedKey',
  Consent: 'NeedKey',
  ChooseModel: 'Consent',
  CheckDocker: 'Consent',
  InstallDockerGuide: 'Consent',
  CheckDaemon: 'Consent',
  StartDaemon: 'CheckDaemon',
  // 自动安装一旦开跑就没有「上一步」——想停下来只能点「停止」（它会走到错误页或运行面板）
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
  // I5：安装期的失败一律回到「自动安装」那一页从头再跑一次 ——
  // 那一页现在就是安装本身，没有别的地方可回
  E_PULL_FAILED: 'AutoInstalling',
  E_PORT_IN_USE: 'AutoInstalling',
  E_PORT_CONFLICT: 'AutoInstalling',
  E_CRED_HELPER: 'AutoInstalling',
  E_START_TIMEOUT: 'AutoInstalling',
  E_BUILTIN_DOWN: 'AutoInstalling',
  E_RUNTIME_NO_DNS: 'AutoInstalling',
  E_CONTAINER_OFFLINE: 'AutoInstalling',
  E_COMPOSE_FETCH: 'AutoInstalling',
  E_CONFIG_WRITE: 'AutoInstalling',
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

  // **RESUME_INSTALL 也是任何状态下都能进**（I9 的 P0-3）。
  //
  // 它不是「用户想重试」，是后端**已经**把安装重新跑起来了 ——
  // 界面这边只是跟上一个既成事实。从错误页进得去，从运行面板也进得去；
  // 已经在那一页上就原地不动（那一页自己会从快照里把事件补齐）。
  if (event.type === 'RESUME_INSTALL') {
    return state.name === 'AutoInstalling' ? state : s('AutoInstalling')
  }

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
      // I5：Docker 这一关过了就直接回到自动安装，不再把用户送回「选模型」
      if (event.type === 'DAEMON_UP') return s('AutoInstalling')
      return state

    case 'StartDaemon':
      if (event.type === 'TRY_START_DAEMON' || event.type === 'RECHECK_DOCKER') return s('CheckDaemon')
      return state

    case 'NeedKey':
      return event.type === 'KEY_SUBMIT' ? s('ValidateKey') : state

    case 'ValidateKey':
      // I5：key 验过之后先做一次授权，再谈别的
      if (event.type === 'KEY_VALID') return s('Consent')
      if (event.type === 'KEY_INVALID') return s('NeedKey')
      return state

    case 'Consent':
      // 授权完就直接开装。选模型是支线：默认走 Hunter 网关，
      // 想换成自带 key 的人在授权页上点那条链接（或者装好之后在设置里改）
      if (event.type === 'CONSENT_GIVEN') return s('AutoInstalling')
      if (event.type === 'CHOOSE_MODEL') return s('ChooseModel')
      return state

    case 'ChooseModel':
      return event.type === 'MODEL_CHOSEN' ? s('AutoInstalling') : state

    case 'AutoInstalling':
      if (event.type === 'AUTO_DONE') return s('Done')
      if (event.type === 'AUTO_FAILED') return fail(state, event.code, event.detail)
      return state

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

export type StepId = 'welcome' | 'key' | 'consent' | 'model' | 'install'
/** 步骤条的顺序。I5：授权页排在 key 之后，装的那几步合成一步「自动安装」。
 *  「选择模型」留在条上（它仍然是向导的一环），但主路径会**跳过**它 ——
 *  跳过之后它显示成「已完成」，因为默认值（Hunter 网关）本来就已经定好了。 */
export const STEP_IDS: StepId[] = ['welcome', 'key', 'consent', 'model', 'install']

const STATE_STEP: Partial<Record<StateName, StepId>> = {
  Welcome: 'welcome',
  NeedKey: 'key',
  ValidateKey: 'key',
  Consent: 'consent',
  ChooseModel: 'model',
  // Docker 的四个状态归到「自动安装」这一步：它们本来就是自动安装的第一件事，
  // 只有 AI 修不了、要用户自己动手时才会真的显示出那一页
  CheckDocker: 'install',
  InstallDockerGuide: 'install',
  CheckDaemon: 'install',
  StartDaemon: 'install',
  AutoInstalling: 'install',
  Starting: 'install',
  Done: 'install',
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
  | 'consent'
  | 'model'
  | 'auto'
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
  Consent: 'consent',
  ChooseModel: 'model',
  AutoInstalling: 'auto',
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
