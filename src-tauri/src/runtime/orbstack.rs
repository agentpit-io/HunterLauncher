//! OrbStack 官方 dmg —— `install_runtime` 的 **macOS 备选路线**（I7）。
//!
//! ## 为什么是备选而不是默认
//!
//! 这条路线本身很短：下 dmg → 验签 → 挂载 → 拷到 `/Applications` → 卸载镜像 →
//! `open -a OrbStack` → 等 daemon。问题出在最后一步之后：
//!
//! > **OrbStack 首次启动会弹系统级提示** —— 欢迎页，以及「安装辅助程序」那一步
//! > 要管理员密码。那是 macOS 与 OrbStack 自己的交互，启动器**无法也不应**代点。
//!
//! 也就是说这条路走不到「零点击」。用户 2026-09-21 17:00 要的是「不要人去操作」，
//! 所以默认路线是 [`super::builtin`]（Colima，全程零点击），这条留在设置里给
//! 「我就想要 OrbStack」的人。报告与界面上都要把这件事**说清楚**，不能含糊。
//!
//! 另外：**OrbStack 个人使用免费，商用需要授权**。界面上注明，不替用户决定。
//!
//! ## 验签：三道，缺一不可
//!
//! 下回来的 dmg 我们**没有**官方校验和可对（OrbStack 不发 `.sha256`，而且
//! `latest` 地址会随版本漂移），所以改成验苹果的签名与公证：
//!
//! | 检查 | 命令 | 不过就拒绝 |
//! |---|---|---|
//! | 代码签名完整 | `codesign --verify --deep --strict` | 是 |
//! | 公证 / Gatekeeper 放行 | `spctl -a -t open --context context:primary-signature` | 是 |
//! | 签名者是不是 OrbStack | `codesign -dv` 输出里的 TeamIdentifier | 是 |
//!
//! 三道都过了才挂载。**不过就删掉，不「先装了再说」。**
//!
//! ## 本轮的实现程度
//!
//! 整条链路的代码都在这里，但**没有 macOS 真机可跑**（GitHub 的 macOS runner
//! 不支持嵌套虚拟化，装 OrbStack 也起不来 daemon），所以报告里标「未验证」。
//! 能在 CI 上跑的只有：下载地址解析、验签命令的构造、Team ID 比对这几段纯函数。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::err::{AppError, AppResult, Code};

/// OrbStack 的 Apple Developer Team ID。**写死**——
/// 「签名有效」不等于「签名的人是 OrbStack」，不比对 Team ID 的验签是假验签。
pub const TEAM_ID: &str = "HUAQ24HBR6";

/// 官方稳定版直链。实测会 307 到 `cdn-updates.orbstack.dev/<arch>/OrbStack_v<版本>_<号>_<arch>.dmg`。
pub fn download_url() -> AppResult<&'static str> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::new(
            Code::NotImplemented,
            "OrbStack 只有 macOS 版。".to_string(),
        ));
    }
    Ok(match std::env::consts::ARCH {
        "aarch64" => "https://orbstack.dev/download/stable/latest/arm64",
        "x86_64" => "https://orbstack.dev/download/stable/latest/amd64",
        other => {
            return Err(AppError::new(
                Code::NotImplemented,
                format!("OrbStack 没有 {other} 架构的包。"),
            ))
        }
    })
}

/// 装到哪儿。`/Applications` 写得进就用它（所有用户都能看到），
/// 写不进就退到 `~/Applications` —— **不提权**。
pub fn install_dir() -> PathBuf {
    let sys = PathBuf::from("/Applications");
    if writable_dir(&sys) {
        return sys;
    }
    crate::paths::home().join("Applications")
}

fn writable_dir(p: &Path) -> bool {
    let probe = p.join(".hunter-launcher-write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 一次验签的结果。**每一条都记原话**，不过时用户要看得到是哪一条没过。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub codesign_ok: bool,
    pub gatekeeper_ok: bool,
    pub team_ok: bool,
    /// 实际读到的 Team ID（读不到就是空）
    pub team_id: String,
    pub notes: Vec<String>,
}

impl Verdict {
    pub fn ok(&self) -> bool {
        self.codesign_ok && self.gatekeeper_ok && self.team_ok
    }
    pub fn one_line(&self) -> String {
        if self.ok() {
            return format!("签名与公证都验过了（Team ID {}）", self.team_id);
        }
        let mut bad = Vec::new();
        if !self.codesign_ok {
            bad.push("代码签名不完整");
        }
        if !self.gatekeeper_ok {
            bad.push("没通过公证检查");
        }
        if !self.team_ok {
            bad.push("签名者不是 OrbStack");
        }
        format!("验签没过：{}", bad.join("、"))
    }
}

/// 从 `codesign -dv` 的输出里取 Team ID。**纯函数**，CI 上能测。
pub fn parse_team_id(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("TeamIdentifier="))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "not set")
}

/// 验一个 `.app` 或 `.dmg` 的签名。**三道全过才返回 ok。**
pub fn verify(target: &Path) -> AppResult<Verdict> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::new(
            Code::NotImplemented,
            "验签用的是 macOS 自带的 codesign / spctl，别的平台上没有。".to_string(),
        ));
    }
    let t = target.to_string_lossy().into_owned();
    let mut notes = Vec::new();

    let cs = crate::proc::run_timeout(
        "/usr/bin/codesign",
        &["--verify", "--deep", "--strict", "--verbose=2", &t],
        Duration::from_secs(180),
    )?;
    let codesign_ok = cs.ok();
    notes.push(format!(
        "codesign --verify --deep --strict → 退出码 {:?}：{}",
        cs.status,
        first_line(&cs.stderr)
    ));

    let dv = crate::proc::run_timeout(
        "/usr/bin/codesign",
        &["-dv", "--verbose=4", &t],
        Duration::from_secs(120),
    )?;
    let team_id = parse_team_id(&dv.stderr)
        .or_else(|| parse_team_id(&dv.stdout))
        .unwrap_or_default();
    let team_ok = team_id == TEAM_ID;
    notes.push(format!(
        "TeamIdentifier = {}（期望 {TEAM_ID}）",
        if team_id.is_empty() {
            "读不到"
        } else {
            &team_id
        }
    ));

    let sp = crate::proc::run_timeout(
        "/usr/sbin/spctl",
        &[
            "-a",
            "-t",
            "open",
            "--context",
            "context:primary-signature",
            "-vv",
            &t,
        ],
        Duration::from_secs(180),
    )?;
    let gatekeeper_ok = sp.ok();
    notes.push(format!(
        "spctl → 退出码 {:?}：{}",
        sp.status,
        first_line(&sp.stderr)
    ));

    Ok(Verdict {
        codesign_ok,
        gatekeeper_ok,
        team_ok,
        team_id,
        notes,
    })
}

/// 挂载 dmg 要跑的命令。挂载点指死在 `~/.hunter/runtime/mnt`，
/// **只读挂载**、不弹 Finder 窗口（`-nobrowse`）。
pub fn attach_argv(dmg: &Path, mount: &Path) -> Vec<String> {
    vec![
        "/usr/bin/hdiutil".into(),
        "attach".into(),
        dmg.to_string_lossy().into_owned(),
        "-nobrowse".into(),
        "-readonly".into(),
        "-mountpoint".into(),
        mount.to_string_lossy().into_owned(),
    ]
}

pub fn detach_argv(mount: &Path) -> Vec<String> {
    vec![
        "/usr/bin/hdiutil".into(),
        "detach".into(),
        mount.to_string_lossy().into_owned(),
        "-force".into(),
    ]
}

/// 整条链路。**每一步失败都如实返回，不跳过、不假装成功。**
///
/// `say` 是进度回调（事件流）。装完**不替用户点开** —— 起它是另一个动作
/// （`start_runtime`），因为 OrbStack 首启的系统提示只能用户自己过。
pub fn install(say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool) -> AppResult<String> {
    let url = download_url()?;
    let dmg = crate::paths::runtime_cache().join("OrbStack.dmg");
    std::fs::create_dir_all(crate::paths::runtime_cache()).ok();
    crate::assist::guard::writable_path(&dmg)?;

    say("正在下载 OrbStack 官方安装镜像");
    // OrbStack 的 dmg 实测在 200 MB 量级；给 600 MB 上限
    let mut last = 0u64;
    let n = crate::http::download_to_file(
        url,
        &dmg,
        Duration::from_secs(1800),
        600 * 1024 * 1024,
        cancel,
        &mut |got, total| {
            if got - last > 8 * 1024 * 1024 {
                last = got;
                match total {
                    Some(t) => say(&format!(
                        "已下载 {} / {}",
                        crate::assist::probe::human_bytes(got),
                        crate::assist::probe::human_bytes(t)
                    )),
                    None => say(&format!(
                        "已下载 {}",
                        crate::assist::probe::human_bytes(got)
                    )),
                }
            }
        },
    )?;
    say(&format!(
        "下载完成（{}）",
        crate::assist::probe::human_bytes(n)
    ));

    say("正在验证苹果签名与公证");
    let v = verify(&dmg)?;
    if !v.ok() {
        let _ = std::fs::remove_file(&dmg);
        return Err(AppError::new(
            Code::Unknown,
            format!(
                "{}。安装镜像已删掉，不会使用。\n{}",
                v.one_line(),
                v.notes.join("\n")
            ),
        ));
    }
    say(&v.one_line());

    let mount = crate::paths::runtime_dir().join("mnt");
    std::fs::create_dir_all(&mount).ok();
    let a = attach_argv(&dmg, &mount);
    crate::assist::guard::argv(&a)?;
    let (prog, args) = a.split_first().expect("非空");
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let r = crate::proc::run_timeout(prog, &refs, Duration::from_secs(300))?;
    if !r.ok() {
        return Err(AppError::new(
            Code::Unknown,
            format!("挂载安装镜像失败：{}", r.err_line()),
        ));
    }

    // 不管后面成不成，镜像都要卸载掉
    let result = copy_app(&mount, say);
    let d = detach_argv(&mount);
    if crate::assist::guard::argv(&d).is_ok() {
        let (p2, a2) = d.split_first().expect("非空");
        let r2: Vec<&str> = a2.iter().map(String::as_str).collect();
        let _ = crate::proc::run_timeout(p2, &r2, Duration::from_secs(120));
    }
    let _ = std::fs::remove_file(&dmg);
    let _ = std::fs::remove_dir(&mount);
    result
}

fn copy_app(mount: &Path, say: &mut dyn FnMut(&str)) -> AppResult<String> {
    let src = mount.join("OrbStack.app");
    if !src.exists() {
        return Err(AppError::new(
            Code::Unknown,
            format!(
                "安装镜像里没有 OrbStack.app（挂载点 {}）。",
                mount.display()
            ),
        ));
    }
    let dir = install_dir();
    std::fs::create_dir_all(&dir).ok();
    let dest = dir.join("OrbStack.app");
    say(&format!(
        "正在拷到 {}",
        crate::redact::mask_home(&dir.to_string_lossy())
    ));
    // `.app` 是一棵目录树，苹果自己的工具（ditto）拷它才不会丢扩展属性与签名
    let argv = vec![
        "/usr/bin/ditto".to_string(),
        src.to_string_lossy().into_owned(),
        dest.to_string_lossy().into_owned(),
    ];
    crate::assist::guard::argv(&argv)?;
    let (prog, args) = argv.split_first().expect("非空");
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let r = crate::proc::run_timeout(prog, &refs, Duration::from_secs(600))?;
    if !r.ok() {
        return Err(AppError::new(
            Code::Unknown,
            format!("拷 OrbStack.app 失败：{}", r.err_line()),
        ));
    }
    // 拷过去之后再验一次 —— 拷贝过程也可能把签名弄坏
    let v = verify(&dest)?;
    if !v.ok() {
        return Err(AppError::new(
            Code::Unknown,
            format!("拷过去之后签名验不过了：{}", v.one_line()),
        ));
    }
    Ok(format!(
        "OrbStack 已装到 {}。{}",
        crate::redact::mask_home(&dest.to_string_lossy()),
        FIRST_RUN_NOTE
    ))
}

/// 这句话必须原样出现在界面与报告里 —— 它是这条路线**做不到零点击**的原因。
pub const FIRST_RUN_NOTE: &str = "第一次打开 OrbStack 时，macOS 与 OrbStack 自己会弹欢迎页，\
并且可能要你输一次管理员密码来安装它的辅助程序。那是系统与 OrbStack 的交互，\
启动器不会、也不应该替你点。另：OrbStack 个人使用免费，商用需要另外授权。";

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_id_解析() {
        let out = "Executable=/Applications/OrbStack.app/Contents/MacOS/OrbStack\n\
                   Identifier=dev.kdrag0n.MacVirt\n\
                   TeamIdentifier=HUAQ24HBR6\n\
                   Sealed Resources version=2\n";
        assert_eq!(parse_team_id(out).as_deref(), Some(TEAM_ID));
        // 没签名的包会写 `not set` —— 那不算读到
        assert_eq!(parse_team_id("TeamIdentifier=not set"), None);
        assert_eq!(parse_team_id("什么都没有"), None);
    }

    /// 「签名有效」不等于「签名的人是 OrbStack」。三道缺一不可。
    #[test]
    fn 三道验签缺一不可() {
        let base = Verdict {
            codesign_ok: true,
            gatekeeper_ok: true,
            team_ok: true,
            team_id: TEAM_ID.into(),
            notes: vec![],
        };
        assert!(base.ok());
        for f in [
            |v: &mut Verdict| v.codesign_ok = false,
            |v: &mut Verdict| v.gatekeeper_ok = false,
            |v: &mut Verdict| v.team_ok = false,
        ] {
            let mut v = base.clone();
            f(&mut v);
            assert!(!v.ok(), "少一道就该判不过");
            assert!(v.one_line().contains("没过"), "{}", v.one_line());
        }
    }

    #[test]
    fn 下载地址按架构分而且不是_http() {
        if cfg!(target_os = "macos") {
            let u = download_url().expect("mac 上要给得出地址");
            assert!(
                u.starts_with("https://orbstack.dev/download/stable/latest/"),
                "{u}"
            );
            assert!(u.ends_with("arm64") || u.ends_with("amd64"), "{u}");
        } else {
            assert!(download_url().is_err(), "非 macOS 上要如实拒绝");
        }
    }

    /// 挂载一律 `-readonly -nobrowse`：不写镜像、不弹 Finder 窗口。
    #[test]
    fn 挂载参数是只读且不弹窗() {
        let a = attach_argv(Path::new("/tmp/x.dmg"), Path::new("/tmp/mnt"));
        assert!(a.contains(&"-readonly".to_string()), "{a:?}");
        assert!(a.contains(&"-nobrowse".to_string()), "{a:?}");
        assert!(a.contains(&"-mountpoint".to_string()), "{a:?}");
        // 守卫也要放行（hdiutil 不在禁用名单里，参数里也没有越界路径）
        assert!(crate::assist::guard::argv(&a).is_ok());
        assert!(crate::assist::guard::argv(&detach_argv(Path::new("/tmp/mnt"))).is_ok());
    }

    /// 首启会弹系统提示这件事**必须**写在文案里 —— 这是这条路线不是默认的原因。
    #[test]
    fn 首启提示里说清了代不了点与商用授权() {
        assert!(FIRST_RUN_NOTE.contains("管理员密码"));
        assert!(FIRST_RUN_NOTE.contains("不会、也不应该替你点"));
        assert!(FIRST_RUN_NOTE.contains("商用"));
    }
}
