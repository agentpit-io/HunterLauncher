//! 升级这条路的自愈（I19 · P0-B）。
//!
//! ## 它和安装那套 AI 自驾是什么关系
//!
//! 同一台机器、同一批角色（侦察员 → 诊断员（规则优先、模型兜底）→ 守卫 →
//! 复核员 → 执行员 → 验证员）。区别只在**步骤枚举**与**回合预算**：
//!
//! * 安装走 `assist::auto::Step`（Docker / Prepare / Pull / Start），
//!   预算是设计文档 §八 那三个数（4 / 10 / 20 万 token）；
//! * 升级走 [`crate::upgrade::UpgradeStep`]（八步），预算——
//!   **整轮升级最多 [`MAX_UPGRADE_REPAIR_ROUNDS`] 个修复回合**，用户 2026-10-05 拍板。
//!
//! 「修复回合」那一段（`repair` / `scout` / `rule_plan` / `ask_model` / `review`）
//! 本身与安装无关 —— I19 把它抽成不依赖安装 `Step` / `InstallOptions` 的中间层
//! （`assist::auto` 里的 [`crate::assist::auto::Stage`] 与
//! [`Orchestrator::repair_round`](crate::assist::auto::Orchestrator::repair_round)），
//! 升级直接复用，**不另抄第二份**。
//!
//! ## 三条不放松的事
//!
//! 1. **规则优先**：规则认得出来就零 token、零等待，同一个现象每次同一句话；
//!    只有规则认不出才把（已脱敏的）证据交给模型，模型只能从固定动作表里挑。
//! 2. **守卫一个字都不松**：宁可停在「修不好」，也不许自愈去删数据卷、动别人的容器、改网络。
//! 3. **验证 = 重跑失败那一步**，不问模型「好了吗」。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::assist::auto::{Budget, Orchestrator, MAX_TOKENS};
use crate::assist::events::Bus;
use crate::assist::guard::Mode;
use crate::err::AppError;
use crate::flow::AppState;
use crate::upgrade::UpgradeStep;

/// **整轮升级最多几个修复回合**（用户 2026-10-05 拍板）。
///
/// **不共用** `assist::auto::MAX_ROUNDS_PER_ISSUE`（4）与 `MAX_ROUNDS_TOTAL`（10）——
/// 那两条是安装的语义。用户的原话是「循环 5 次异常无法解决后才主动停止」，
/// 按**整轮升级**算：一次升级里任何一步失败触发的自愈都算进同一个计数器。
pub(crate) const MAX_UPGRADE_REPAIR_ROUNDS: usize = 5;

/// 整轮升级共用一个修复回合计数器。**任何一步失败触发的自愈都从它这儿取号**，
/// 用满 [`MAX_UPGRADE_REPAIR_ROUNDS`] 即停（用户拍板的口径）。
///
/// 单独抽成类型是为了**考得了**：A7② 那条单测直接对它取 6 次号，
/// 断言第 5 次是最后一次、第 6 次为空。
#[derive(Debug, Default)]
pub(crate) struct RepairBudget {
    used: usize,
}

impl RepairBudget {
    /// 取一个回合号。用满之后返回 `None`（= 该停下了）。
    pub(crate) fn take(&mut self) -> Option<usize> {
        if self.used >= MAX_UPGRADE_REPAIR_ROUNDS {
            return None;
        }
        self.used += 1;
        Some(self.used)
    }

    pub(crate) fn used(&self) -> usize {
        self.used
    }
}

/// 一次修复回合的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepairVerdict {
    /// 做了点什么 —— **重跑失败那一步就是验证**
    Repaired,
    /// 5 个回合用满了
    Exhausted,
    /// 规则与模型都没辙，或复核员连续否决 —— 不该修 / 修不了
    Rejected,
}

/// 升级那条路上的一次自愈会话。**一轮升级共用一个**，回合计数也就共用一个。
pub struct UpgradeRepairer {
    orch: Orchestrator,
    budget: RepairBudget,
    /// **是因为 5 个回合用满才停的**吗（区别于「守卫判定不该修」）。
    /// 界面要把这两件事分开说：一个是「试到头了」，一个是「这条路根本不该走」。
    stopped_exhausted: bool,
}

impl UpgradeRepairer {
    /// 建一次自愈会话。
    ///
    /// `mode` / `key` 与安装那条路同源（用户在设置里选的档位、`hunter_key`）。
    /// **AI 关着也不影响**：规则层是确定性的，`mode == Off` 时它照样给出计划，
    /// 只是模型那一层不问 —— 这正是「规则优先」的意义。
    pub(crate) fn new(
        bus: Arc<Bus>,
        mode: Mode,
        key: Option<String>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let mut orch = Orchestrator::new(bus, mode, key, cancel);
        // 升级的回合预算：整轮 5 个（见文件头）。token 上限沿用安装那份 ——
        // 它防的是「模型被反复问」，与步骤语义无关。
        orch.set_budget(Budget {
            per_issue: MAX_UPGRADE_REPAIR_ROUNDS,
            total: MAX_UPGRADE_REPAIR_ROUNDS,
            tokens: MAX_TOKENS,
        });
        Self {
            orch,
            budget: RepairBudget::default(),
            stopped_exhausted: false,
        }
    }

    pub(crate) fn rounds_used(&self) -> usize {
        self.budget.used()
    }

    /// **是不是 5 个回合用满才停的**（区别于守卫/规则判定「不该修」）。
    pub(crate) fn stopped_exhausted(&self) -> bool {
        self.stopped_exhausted
    }

    /// 升级的某一步失败了：给它一个修复回合。
    ///
    /// 返回 [`RepairVerdict::Repaired`] 时调用方**重跑失败那一步**就算验证；
    /// [`RepairVerdict::Exhausted`] / [`RepairVerdict::Rejected`] 时该停下。
    pub(crate) fn repair(
        &mut self,
        state: &AppState,
        step: UpgradeStep,
        e: &AppError,
    ) -> RepairVerdict {
        let Some(round) = self.budget.take() else {
            self.stopped_exhausted = true;
            return RepairVerdict::Exhausted;
        };
        crate::linfo!(
            "升级自愈：第 {round}/{MAX_UPGRADE_REPAIR_ROUNDS} 个修复回合，这一步是「{}」，错误 {}",
            step.title(),
            e.code.as_str()
        );
        if self.orch.repair_round(state, step.stage(), e, round) {
            RepairVerdict::Repaired
        } else {
            RepairVerdict::Rejected
        }
    }
}

/// 「修不好」停下时，配置到底动过没有 —— **纯函数**，A7⑤ 钉的就是它。
///
/// 第 ③ 步（写配置）之后失败，回滚那一步必须走（配置已经改过了）；
/// 在此之前失败，本来就什么都没改，**如实说「什么都没改」**，不许谎称回滚过。
pub(crate) fn stopped_config_state(step: UpgradeStep) -> &'static str {
    if step.past_config_write() {
        "配置已经改过，正在写回升级前那一份"
    } else {
        "配置一个字节都没改"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assist::guard::Proposer;

    /// A7①：用户拍板的数字是 5，谁改了这个常量先红。
    #[test]
    fn 整轮升级最多五个修复回合() {
        assert_eq!(MAX_UPGRADE_REPAIR_ROUNDS, 5);
    }

    /// A7②：**整轮计数**——第 5 次是最后一次，第 6 次就该停下。
    #[test]
    fn 整轮计数第五次用满即停() {
        let mut b = RepairBudget::default();
        assert_eq!(b.take(), Some(1));
        assert_eq!(b.take(), Some(2));
        assert_eq!(b.take(), Some(3));
        assert_eq!(b.take(), Some(4));
        assert_eq!(b.take(), Some(5), "第 5 次是最后一次修复回合");
        assert_eq!(b.take(), None, "第 6 次必须停下");
        assert_eq!(b.used(), 5, "用满之后计数不再涨");
    }

    /// A7⑤：停下时「配置动过没有」——过第 ③ 步的必回滚，没过的如实说「什么都没改」。
    #[test]
    fn 停下时按有没有过第3步决定回不回滚() {
        // 第 ③ 步及之后：配置已改，必须回滚
        for s in [
            UpgradeStep::WriteConfig,
            UpgradeStep::ConfigCheck,
            UpgradeStep::Pull,
            UpgradeStep::Up,
        ] {
            assert!(s.past_config_write(), "{s:?} 在写配置之后，该回滚");
            assert!(
                stopped_config_state(s).contains("回"),
                "{s:?} 的收尾话里要说清在写回"
            );
        }
        // 第 ③ 步之前：本来就什么都没动，如实说
        for s in [
            UpgradeStep::RuntimeReady,
            UpgradeStep::FetchCompose,
            UpgradeStep::Preflight,
            UpgradeStep::Backup,
        ] {
            assert!(!s.past_config_write(), "{s:?} 在写配置之前，没有可回滚的");
            assert!(stopped_config_state(s).contains("都没改"), "{s:?}");
        }
    }

    /// A7③：**规则优先**——规则认得出来时一个 token 都不花，不去问模型。
    ///
    /// 做法：建一个 `mode = Auto` 但**没有 key** 的总指挥。规则层给出的计划
    /// 走的是 `Proposer::Rule`；真掉到模型那一层，没有 key 只会返回 `None`。
    /// 所以「拿到了计划且 by == Rule」本身就证明了模型那一层没参与。
    ///
    /// 选 `E_RUNTIME_NO_DNS`（虚拟机解析不了域名）当样本：它的规则是纯本地的
    /// （不联网、不碰 docker），单测里跑得又快又稳。
    #[test]
    fn 规则认得出时不问模型() {
        use crate::assist::auto::{Evidence, Step};
        use crate::assist::probe;
        let mut o = Orchestrator::new(
            std::sync::Arc::new(Bus::new(Box::new(crate::assist::events::Null), false)),
            Mode::Auto,
            None, // 没有 key —— 一旦走到模型那一层就只会返回 None
            std::sync::Arc::new(AtomicBool::new(false)),
        );
        let e = AppError::new(crate::err::Code::RuntimeNoDns, "虚拟机解析不了域名（测试）");
        let mut ev = Evidence {
            report: probe::collect(Some("E_RUNTIME_NO_DNS"), Some("测试"), Some("start")),
            others: Vec::new(),
            stale: Vec::new(),
            port_lines: Vec::new(),
            cred_helpers: Vec::new(),
            sub_env: String::new(),
            container_net: None,
            stale_dns: Vec::new(),
            notes: Vec::new(),
        };
        match o.plan_for(0, Step::Start.stage(), &e, &mut ev, 1) {
            Some((_calls, _why, by)) => {
                assert_eq!(by, Proposer::Rule, "规则认得出来就该由规则判，不该去问模型")
            }
            None => panic!("规则层应当认得出 E_RUNTIME_NO_DNS"),
        }
        assert_eq!(o.tokens(), 0, "规则优先：一个 token 都不许花");
    }

    /// A7④：**守卫拒绝越界动作**。动作表外的一律拒绝；删数据卷、改 hosts 这类
    /// 危险命令根本不在表里，模型连「提出」的机会都没有。
    #[test]
    fn 守卫拒绝越界动作() {
        use crate::assist::actions::{self, Call};
        // 三种都是模型**可能**想提、但表里刻意没有的动作
        for bad in [
            // 绝不能删数据卷
            Call::with("run_command", "cmd", "docker volume rm hunter_pgdata"),
            // 绝不能删用户自己的文件
            Call::with("run_command", "cmd", "rm -rf /home/user/Documents"),
            // 绝不能改 hosts
            Call::with("run_command", "cmd", "echo 1.2.3.4 x >> /etc/hosts"),
            // 表外的一切（含换个名字的同类）
            Call::new("docker_volume_rm"),
            Call::new("delete_user_files"),
            Call::new("edit_hosts"),
        ] {
            let r = actions::plan(&bad);
            assert!(
                r.is_err(),
                "越界动作「{}」必须被动作表拒掉，实际却放行了",
                bad.id
            );
        }
        // 反面：表内的动作照样通过（证明拒绝不是因为「全都不让做」）
        assert!(actions::plan(&Call::new("check_ports")).is_ok());
    }
}
