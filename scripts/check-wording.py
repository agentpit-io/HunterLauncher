#!/usr/bin/env python3
"""界面文案里不许出现「让用户自己去操作」的话（I8 · 任务书〇.4 与〇·五.7）。

## 为什么要这么一条检查

用户 2026-09-21 22:10 定了一条原则：

> 所有能替用户解决的问题——分析异常、执行操作——都不要让用户自己去操作。

2026-09-21 22:35 又加了一条：

> 不要让用户参与决策和点击确认和执行，出现问题，自主分析，按最佳方案执行。

**这两条靠提示词是守不住的。** 0.1.7 在用户 Mac 上停在一张
「请在终端里执行：brew install --cask orbstack」+「我执行完了，继续」的卡片上，
而那张卡片是代码里写死的字符串 —— 不是模型编的。所以守它的办法只能是
**在构建流水线上扫字符串**：会上屏的文案里出现这类话，CI 直接红。

## 扫什么、不扫什么

| 扫 | 不扫 |
|---|---|
| `src/i18n/*.ts`（界面全部文案） | 注释（注释不会上屏） |
| `src/pages` / `src/components` 里的中英文字面量 | `--help` 与命令行输出（本来就是在终端里看的） |
| 规则层 `assist/rules.rs` 的说明 | 日志与诊断包（给开发者看，见白名单） |
| 动作表 `assist/actions.rs` 的标题与理由 | 文档 `.md`（里面会**故意**引用这些话来说明改了什么） |
| 总指挥 `assist/auto.rs`、兜底链等会 emit 到事件流的字符串 | 单测里故意造的反例（逐行豁免标记） |

Rust 那一侧只取**字符串字面量**（复用 `rust_strings.py`）—— 这一整套文档与注释里
到处都在讨论「终端」「请在终端里执行」，把注释也扫进来的话检查第一天就没法用。

## 逐行豁免

实在需要写出这些字眼的地方（这份脚本自己的名单、一条反例单测、或者
**交给模型的提示词里那句「你不要让用户去终端里敲命令」**），加注释标记
`wording-ok`。逐行而不是整文件豁免 —— 整文件豁免会把一个文件变成永久盲区
（`check-retired-mirrors.sh` 里同一条教训）。

标记认**命中行前后各 3 行**，而不是只认命中行本身：Rust 的多行原始字符串里
没法插注释，只能把标记写在字符串开头那几行的上面。范围给 3 行而不是整个字面量，
是为了不让一个几十行的提示词常量顺带把后面的内容也豁免掉。
"""
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from rust_strings import string_literals  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent

# ── 名单 ──────────────────────────────────────────────────────────────────
#
# 每一条都写清「它长什么样」与「为什么不许出现」。正则而不是裸子串，是因为
# 有些词单独出现是正当的：「设置页里点一下就能卸干净」讲的是这东西多好卸，
# 不是在安装过程中支使用户 —— 所以拦的是「点一下继续 / 点一下确认」这种句式。
RULES = [
    (
        r"(在|去|到)?\s*(终端|命令行|Terminal|iTerm|控制台)\s*(里|中|下|窗口)?\s*"
        r"(执行|运行|敲|输入|跑|打开|粘贴)",
        "让用户去终端里敲命令 —— 启动器自己执行，做不了就如实说做不了",
    ),
    (
        r"(复制|拷贝)\s*(这|下面|上面|以下)?\s*(条|行|段|个)?\s*(命令|指令|脚本)",
        "让用户复制命令 —— 同上",
    ),
    (
        r"(我)?\s*(已经)?\s*执行完了|我装好了|我弄好了|做完了，?\s*继续",
        "「我执行完了，继续」这一类按钮 —— 活儿不该丢回给用户",
    ),
    (
        r"(请|你需要|需要你|麻烦你)\s*(先)?\s*(手动|自己|手工)\s*(执行|运行|安装|装|改|配置)",
        "让用户手动操作",
    ),
    (
        r"(请你|需要你|请先)\s*(选择|选一个|决定|确认|确定)|你来(决定|选)",
        "让用户做决策 —— 全自动档下由守卫 + 复核员把关后自己拿主意",
    ),
    (
        r"点(一下|击|下)\s*(这里|下面|上面|按钮)?\s*(继续|确认|开始|重试|同意)",
        "让用户点一下才往下走 —— 安装过程里不该有需要点的按钮",
    ),
    (
        r"(请|建议你|你可以)\s*(参照|照着|按照)?\s*(官方)?\s*(文档|说明|教程)\s*(自行|自己)",
        "把用户打发去看文档自己弄",
    ),
]

# ── 扫哪些文件 ────────────────────────────────────────────────────────────
TS_GLOBS = ["src/i18n/*.ts", "src/pages/*.tsx", "src/components/*.tsx"]
RS_FILES = [
    "src-tauri/src/assist/rules.rs",
    "src-tauri/src/assist/actions.rs",
    "src-tauri/src/assist/auto.rs",
    "src-tauri/src/assist/probe.rs",
    "src-tauri/src/assist/reviewer.rs",
    "src-tauri/src/assist/guard.rs",
    "src-tauri/src/runtime/chain.rs",
    "src-tauri/src/runtime/brew.rs",
    "src-tauri/src/runtime/elevate.rs",
    "src-tauri/src/runtime/builtin.rs",
    "src-tauri/src/runtime/orbstack.rs",
    "src-tauri/src/netproxy.rs",
    "src-tauri/src/flow.rs",
    "src-tauri/src/err.rs",
    "src-tauri/src/selfupdate.rs",
]
# 白名单：这几处的「终端」是正当的 —— 它们**本来就在终端里**，或者只给开发者看。
#   headless.rs  命令行入口，输出就是打印到终端的
#   log.rs / diag  本地日志与脱敏诊断包，给开发者与 issue 用
# 它们不在上面的 RS_FILES 里，这里写出来是为了说明「为什么没扫」。
EXEMPT = "wording-ok"


def hits(text: str):
    for pat, why in RULES:
        m = re.search(pat, text)
        if m:
            return m.group(0), why
    return None


NEAR = 3  # 豁免标记的作用范围：命中行前后各几行


def exempted(lines, n: int) -> bool:
    """第 `n` 行（1 起）前后 NEAR 行内有没有豁免标记。"""
    lo, hi = max(1, n - NEAR), min(len(lines), n + NEAR)
    return any(EXEMPT in lines[i - 1] for i in range(lo, hi + 1))


def scan_ts(path: Path):
    out = []
    lines = path.read_text(encoding="utf-8").splitlines()
    for n, line in enumerate(lines, 1):
        if exempted(lines, n):
            continue
        # 去掉整行注释；行尾注释里的字眼不上屏，但为简单起见一起扫（误报可加豁免标记）
        if line.lstrip().startswith("//") or line.lstrip().startswith("*"):
            continue
        h = hits(line)
        if h:
            out.append((n, h[0], h[1], line.strip()[:110]))
    return out


def scan_rs(path: Path):
    src = path.read_text(encoding="utf-8")
    lines = src.splitlines()
    out = []
    for lit in string_literals(src):
        h = hits(lit)
        if not h:
            continue
        # 命中的那个片段在源文件第几行 —— 既为了报错时能定位，也为了判豁免。
        # 片段可能被 Rust 的 `\` 续行拆开，所以找不到时退回到整个字面量的首行。
        n = next((i for i, l in enumerate(lines, 1) if h[0] in l), None)
        if n is None:
            head = lit.strip().splitlines()[0][:30]
            n = next((i for i, l in enumerate(lines, 1) if head and head in l), 1)
        if exempted(lines, n):
            continue
        out.append((n, h[0], h[1], lit.strip()[:110]))
    return out


# ── 自测：证明这份检查不是空跑 ────────────────────────────────────────────
#
# 任务书要求「故意加一条『请在终端里执行』验证检查会失败，再删掉」。
# 与其靠人手动加一次再删掉（下次谁也不会再验一遍），不如把正反例钉成自测：
# 每次 CI 都会跑，规则被谁改松了当场就红。
#
# 下面这些字符串是**故意违规**的样本，所以整个 SELF_TEST 块逐行带豁免标记。
BAD_SAMPLES = [
    "请在终端里执行：brew install --cask orbstack",  # wording-ok（反例样本）
    "我执行完了，继续",  # wording-ok（反例样本）
    "复制这条命令到命令行里跑一下",  # wording-ok（反例样本）
    "请你选择要用哪一套",  # wording-ok（反例样本）
    "装好之后点一下继续",  # wording-ok（反例样本）
    "需要你确认一下这一步",  # wording-ok（反例样本）
    "请先手动安装 Docker",  # wording-ok（反例样本）
    "请参照官方文档自行处理",  # wording-ok（反例样本）
    "去 Terminal 里粘贴这段脚本",  # wording-ok（反例样本）
]
# 下面这些**不该**被判违规 —— 规则要是写得太宽，这里会红。
GOOD_SAMPLES = [
    "设置页里点一下就能卸干净",
    "正在下载 OrbStack 官方安装包，约 200 MB",
    "已沿用你设置的网络代理",
    "这一步要往系统目录里写文件，macOS 会弹出它自己的密码框",
    "已自动解决 3 个问题",
    "启动器不会停它、不会删它、不会改它的配置",
    "把日志复制走发给开发者",
    "选择模型",
]


def self_test() -> int:
    bad = []
    for x in BAD_SAMPLES:
        if not hits(x):
            bad.append(f"应当判违规却放过了：{x!r}")
    for x in GOOD_SAMPLES:
        h = hits(x)
        if h:
            bad.append(f"误判了正当文案：{x!r}（命中 {h[0]!r} · {h[1]}）")
    if bad:
        print("check-wording 自测没过：")
        for b in bad:
            print(f"  · {b}")
        return 1
    print(
        f"OK：自测通过（{len(BAD_SAMPLES)} 条反例全部被拦下，"
        f"{len(GOOD_SAMPLES)} 条正当文案一条都没误判）"
    )
    return 0


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
    bad = []
    files = []
    for g in TS_GLOBS:
        files += sorted(ROOT.glob(g))
    for f in files:
        for n, frag, why, ctx in scan_ts(f):
            bad.append((f, n, frag, why, ctx))
    for rel in RS_FILES:
        p = ROOT / rel
        if not p.exists():
            print(f"⚠ 名单里的 {rel} 不存在了，请更新 check-wording.py")
            return 2
        files.append(p)
        for n, frag, why, ctx in scan_rs(p):
            bad.append((p, n, frag, why, ctx))

    if bad:
        print("界面文案里出现了「让用户自己动手」的话（I8 原则）：\n")
        for f, n, frag, why, ctx in bad:
            rel = f.relative_to(ROOT)
            print(f"  {rel}:{n}")
            print(f"    命中：{frag!r}")
            print(f"    为什么不行：{why}")
            print(f"    上下文：{ctx}")
            print()
        print(
            "改法：把这件事做成启动器自己执行的一步（要管理员权限就走系统原生密码框，\n"
            "见 runtime/elevate.rs）；确实做不了就如实说做不了，并给诊断包那条路。\n"
            f"确实必须写出这几个字（反例单测、名单本身），在那一行加注释标记 {EXEMPT}。"
        )
        return 1

    print(f"OK：扫了 {len(files)} 个文件，没有「让用户自己动手」的文案")
    return 0


if __name__ == "__main__":
    sys.exit(main())
