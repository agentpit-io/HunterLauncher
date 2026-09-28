# R2 证据

> 机器：`mixplode-hk-01`（Ubuntu 24.04 · Linux 6.8.0-1066-gcp）
> 时间：2026-09-29（上海时间）

**这一轮在 Linux 上验得了的都验了；Windows 行为一条都没验**（清单在
[R2-迭代报告.md](../R2-迭代报告.md) 第五节）。下面每一份都写清了它证明了什么、
以及它**不**能证明什么 —— 没有一份是在 Linux 上伪造的 Windows 场景。

| 文件 | 是什么 | 证明了 | **不**能证明 |
|---|---|---|---|
| `a-补跑判据-纯函数单测.log` | `cargo test --locked fallback::` 与 `backup::tests::` 的原始输出 | 「该不该补跑」这条判据 18 条测试全过（119/120 分钟边界、24 小时边界、从未成功过、等待时长改成 5、跨天、时钟回拨、开关关掉、Kind 往返、锁跳过） | 判据本身是平台无关的纯函数，所以这一份在哪里跑都成立 —— 但**「真 Windows 上补跑会不会真的跑起来」不在其中** |
| `b-跨进程锁.log` | 用真二进制跑 `--backup --missed` 两次的原始输出 | 跨进程锁这条**跨平台**的真代码路径：锁被别人拿着时返回「跳过」且退出码 0（不是失败）；锁让开之后真的往下走；Drop 会删掉锁文件 | **不涉及 Windows**。另外第二次退出码 1 是因为本机没有 Hunter 那套 compose 文件，与锁无关 |
| `c-补跑写下的meta是missed.json` | `--backup --missed` 真的写到盘上的 `meta.json` | `--backup --missed` 产生的备份档**真的是 `"kind": "missed"`**，不会冒充 `scheduled` | 这份备份本身没做成（本机没有 Hunter 那套服务，`dumpError` 里写着原因）—— 它证明的是**档位**，不是「备份成功」 |
| `d-三个新配置项落盘.log` | 启动器自己写出来的 `launcher.toml` 的 `[backup]` 段 | 三个新配置项（`windows_fallback` / `fallback_delay_mins` / `fallback_interval_hours`）跟着配置一起落盘、读得回来，默认值就是 120 / 24 / true | 用户在设置页改了之后**界面上的表现**没验（那要 GUI 或真 Windows） |

## 跑这些证据时的现场

```bash
export HUNTER_HOME=/tmp/r2-home          # 不碰测试机上真的 ~/.hunter
export DOCKER_HOST=tcp://127.0.0.1:1     # 死端口：读不到 docker，本机一套容器都不会被动到
```

`DOCKER_HOST` 指到一个没人听的端口，在 docker 客户端看来与「daemon 没在跑」是同一件事
（原话就是 `Is the docker daemon running?`）—— 这是 R1 立下的做法，
**不是伪造 Windows，是制造一个「docker 连不上」的真现场**。

备份目录被显式指到 `/tmp/r2-backups`，免得在测试机的家目录下留东西。
