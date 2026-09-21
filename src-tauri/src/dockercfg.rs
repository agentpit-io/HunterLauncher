//! docker 客户端配置（`~/.docker/config.json`）—— **只读用户那一份，另写我们自己那一份**。
//!
//! ## 背景（I6 的 P0）
//!
//! 0.1.5 在用户 Mac 上真机验收时，`docker compose pull` 报：
//!
//! ```text
//! error getting credentials - err: exec: "docker-credential-osxkeychain":
//!   executable file not found in $PATH, out: ``
//! ```
//!
//! 用户的 `~/.docker/config.json` 里有 `"credsStore": "osxkeychain"`
//! （OrbStack / Docker Desktop 装的时候自己写的），docker 每次拉镜像都要先去
//! `$PATH` 上执行 `docker-credential-osxkeychain` 拿凭据；受限 PATH 下找不到，
//! 于是**连公开镜像都拉不动**——这是本机配置的问题，和下载源一点关系都没有。
//!
//! ## 两条处置，顺序不能反
//!
//! 1. **先补 PATH**（[`crate::runtime::env`]）。绝大多数机器上助手就在
//!    `/usr/local/bin` / OrbStack 的 xbin / `~/.docker/bin` 里，补上就好了，
//!    用户那份配置一个字节都不用动。
//! 2. **补完还是找不到**，才走这里：在 `~/.hunter/docker-config/` 里另起一份
//!    **不带 credsStore 的最小配置**，用 `DOCKER_CONFIG` 指过去。
//!
//! ## 为什么不去改用户的 `~/.docker/config.json`
//!
//! 那是**用户的文件**，`~/.hunter` 之外的东西启动器一概不碰（授权页上写死的承诺，
//! [`crate::assist::guard::writable_path`] 会硬拦）。而且改了它会影响用户自己
//! `docker login` 过的所有私有仓库 —— 我们只是要拉六个**公开**镜像，不该有这么大的副作用。
//!
//! ## 那份最小配置里有什么
//!
//! * 用户配置里除 `credsStore` / `credHelpers` / `auths` 之外的键**原样带过来**；
//! * `contexts` 与 `cli-plugins` 两个目录做软链指回原处 —— 不带 `contexts` 的话
//!   `currentContext`（Docker Desktop 上是 `desktop-linux`，OrbStack 上是 `orbstack`）
//!   会找不到，docker 会去连默认的 `/var/run/docker.sock`；不带 `cli-plugins` 的话
//!   `docker compose` 这个插件本身就没了。两个都链不过去时**带不过去哪一项就删哪一项**，
//!   并如实报出来。

use std::path::{Path, PathBuf};

use crate::err::{AppError, AppResult, Code};

/// 用户那份 docker 配置所在的目录。`DOCKER_CONFIG` 存在时用它，否则 `~/.docker`。
pub fn user_dir() -> PathBuf {
    if let Ok(v) = std::env::var("DOCKER_CONFIG") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    crate::paths::home().join(".docker")
}

/// 我们自己那一份放哪。固定在 `~/.hunter/docker-config/`（守卫允许写的范围内）。
pub fn isolated_dir() -> PathBuf {
    crate::paths::root().join("docker-config")
}

/// 读出来的用户配置里与凭据有关的部分。**不读 `auths` 里的任何值**（那是凭据本身）。
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredSetup {
    /// 配置文件在不在
    pub present: bool,
    /// `credsStore` 的值，例如 `osxkeychain` / `desktop` / `secretservice`
    pub creds_store: Option<String>,
    /// `credHelpers` 里出现过的助手名（按仓库配的那些）
    pub cred_helpers: Vec<String>,
}

impl CredSetup {
    /// 这台机器上要用到的助手可执行文件名，去重。
    pub fn helper_programs(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for h in self
            .creds_store
            .iter()
            .cloned()
            .chain(self.cred_helpers.iter().cloned())
        {
            let name = helper_program(&h);
            if !v.contains(&name) {
                v.push(name);
            }
        }
        v
    }
}

/// `osxkeychain` → `docker-credential-osxkeychain`。docker 自己就是这么拼的。
pub fn helper_program(store: &str) -> String {
    format!("docker-credential-{}", store.trim())
}

/// 读用户那份配置里的凭据设置。读不到就是读不到，不猜。
pub fn read_user() -> CredSetup {
    let p = user_dir().join("config.json");
    let Ok(s) = std::fs::read_to_string(&p) else {
        return CredSetup::default();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else {
        crate::lwarn!("{} 不是合法 JSON，按「没有凭据助手」处理", p.display());
        return CredSetup {
            present: true,
            ..Default::default()
        };
    };
    let creds_store = v
        .get("credsStore")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
    let cred_helpers = v
        .get("credHelpers")
        .and_then(|x| x.as_object())
        .map(|o| {
            let mut v: Vec<String> = o
                .values()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
                .collect();
            v.sort();
            v.dedup();
            v
        })
        .unwrap_or_default();
    CredSetup {
        present: true,
        creds_store,
        cred_helpers,
    }
}

/// 配置里要用的助手，**在补全后的 PATH 上**有没有找不到的。
///
/// 返回 `(助手名, 找到的绝对路径或 None)` 的清单。界面与诊断报文照这个说话。
pub fn helper_status() -> Vec<(String, Option<String>)> {
    read_user()
        .helper_programs()
        .into_iter()
        .map(|name| {
            let found = crate::runtime::env::find_on_subprocess_path(&name)
                .map(|p| p.to_string_lossy().into_owned());
            (name, found)
        })
        .collect()
}

/// 补全 PATH 之后**仍然**找不到的助手。空 = 这条路已经走通了。
pub fn missing_helpers() -> Vec<String> {
    helper_status()
        .into_iter()
        .filter(|(_, found)| found.is_none())
        .map(|(n, _)| n)
        .collect()
}

/// 开着隔离配置吗；开着而且那个目录真在，才返回路径。
///
/// 这个函数会被 [`crate::runtime::env`] 在**每次建子进程**时问到，所以只做
/// 读配置 + 判目录两件事，不写任何东西。
pub fn isolated_dir_if_enabled() -> Option<PathBuf> {
    if !crate::config::LauncherConfig::load()
        .runtime
        .isolated_docker_config
    {
        return None;
    }
    let d = isolated_dir();
    if d.join("config.json").is_file() {
        Some(d)
    } else {
        None
    }
}

/// 一次「另起一份配置」的结果，给审计与界面看。
#[derive(Debug, Clone)]
pub struct Isolated {
    pub dir: PathBuf,
    /// 从用户配置里**去掉**的键
    pub dropped: Vec<String>,
    /// 成功链过去的目录
    pub linked: Vec<String>,
    /// 没链过去的目录 + 原因（如实写，不粉饰）
    pub not_linked: Vec<String>,
}

impl Isolated {
    pub fn one_line(&self) -> String {
        let mut s = format!(
            "已在 {} 另起一份不带凭据助手的 docker 配置",
            crate::redact::mask_home(&self.dir.to_string_lossy())
        );
        if !self.dropped.is_empty() {
            s.push_str(&format!("（去掉了 {}）", self.dropped.join("、")));
        }
        if !self.linked.is_empty() {
            s.push_str(&format!("；{} 链回原处", self.linked.join("、")));
        }
        if !self.not_linked.is_empty() {
            s.push_str(&format!("；没能带过来：{}", self.not_linked.join("、")));
        }
        s
    }
}

/// 凭据相关的三个键 —— 这三个不进我们那份配置。
const CRED_KEYS: [&str; 3] = ["credsStore", "credHelpers", "auths"];

/// 要链回原处的目录。`contexts` 关系到连哪个 daemon，`cli-plugins` 关系到还有没有
/// `docker compose` —— 两个都不是可有可无的。
const LINK_DIRS: [&str; 2] = ["contexts", "cli-plugins"];

/// 生成（或刷新）`~/.hunter/docker-config/`。**只写 `~/.hunter` 之内**。
pub fn ensure_isolated() -> AppResult<Isolated> {
    let dst = isolated_dir();
    std::fs::create_dir_all(&dst)
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("建 {} 失败：{e}", dst.display())))?;
    // 守卫：落点必须在 ~/.hunter 内（解软链、解 `..` 之后再判）
    let dst = crate::assist::guard::writable_path(&dst)?;

    let src = user_dir();
    let mut root = std::fs::read_to_string(src.join("config.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    let mut dropped: Vec<String> = Vec::new();
    for k in CRED_KEYS {
        if root.remove(k).is_some() {
            dropped.push(k.to_string());
        }
    }

    let mut linked: Vec<String> = Vec::new();
    let mut not_linked: Vec<String> = Vec::new();
    for d in LINK_DIRS {
        let from = src.join(d);
        if !from.is_dir() {
            continue; // 原来就没有，不算失败
        }
        let to = dst.join(d);
        match link_dir(&from, &to) {
            Ok(()) => linked.push(d.to_string()),
            Err(e) => not_linked.push(format!("{d}（{e}）")),
        }
    }

    // `currentContext` 只有在 contexts 真带过来了的时候才留 ——
    // 留着一个指不到的 context，docker 会直接报 "context not found"，
    // 那就从「拉不动」变成「连不上 daemon」，更糟
    if root.contains_key("currentContext") && !linked.iter().any(|x| x == "contexts") {
        root.remove("currentContext");
        dropped.push("currentContext".into());
    }

    let body = serde_json::to_string_pretty(&serde_json::Value::Object(root))
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("序列化 docker 配置失败：{e}")))?;
    let f = dst.join("config.json");
    std::fs::write(&f, body.as_bytes())
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写 {} 失败：{e}", f.display())))?;
    crate::paths::chmod_600(&f)?;

    let out = Isolated {
        dir: dst,
        dropped,
        linked,
        not_linked,
    };
    crate::linfo!("{}", out.one_line());
    Ok(out)
}

/// 把 `from` 这个目录在 `to` 处做成软链。已经是指向同一处的软链就当成功。
fn link_dir(from: &Path, to: &Path) -> Result<(), String> {
    if let Ok(t) = std::fs::read_link(to) {
        if t == from {
            return Ok(());
        }
        std::fs::remove_file(to).map_err(|e| e.to_string())?;
    } else if to.exists() {
        // 不是软链却占着位置：不删用户可能放在这儿的东西，如实报出来
        return Err("目标已存在且不是软链".to_string());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(from, to).map_err(|e| e.to_string())
    }
    #[cfg(windows)]
    {
        // Windows 上建目录软链要开发者模式或管理员权限；失败就如实说，
        // **不替用户提权**（授权页第 4 条）
        std::os::windows::fs::symlink_dir(from, to)
            .map_err(|e| format!("{e}（Windows 上建目录软链要开发者模式）"))
    }
}

/// 打开隔离配置：生成目录 + 写进 `launcher.toml` + 让子进程环境重新算一遍。
pub fn enable() -> AppResult<Isolated> {
    let out = ensure_isolated()?;
    let mut cfg = crate::config::LauncherConfig::load();
    cfg.runtime.isolated_docker_config = true;
    cfg.save()?;
    crate::runtime::env::invalidate();
    crate::runtime::which::invalidate();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("hunter-dockercfg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// 用一个临时目录冒充用户的 `~/.docker`，同时把 `HUNTER_HOME` 指到临时目录。
    /// 单测**绝不碰真实的 `~/.docker` 与 `~/.hunter`**。
    struct Scene {
        _guard: std::sync::MutexGuard<'static, ()>,
        src: PathBuf,
        home: PathBuf,
    }

    // 这里原来是模块私有的一把锁。它挡得住 dockercfg 自己的测试互相踩，
    // 挡不住 config / telemetry 那边同时改 HUNTER_HOME —— 全进程只能有一把
    // （`paths::env_lock`，那儿的注释写了这次是怎么翻车的）。

    impl Scene {
        fn new(tag: &str) -> Self {
            let g = crate::paths::env_lock();
            let src = tmp(&format!("src-{tag}"));
            let home = tmp(&format!("home-{tag}"));
            std::env::set_var("DOCKER_CONFIG", &src);
            std::env::set_var("HUNTER_HOME", &home);
            Self {
                _guard: g,
                src,
                home,
            }
        }
    }

    impl Drop for Scene {
        fn drop(&mut self) {
            std::env::remove_var("DOCKER_CONFIG");
            std::env::remove_var("HUNTER_HOME");
            let _ = std::fs::remove_dir_all(&self.src);
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    #[test]
    fn 读得出_credsstore_与_credhelpers() {
        let s = Scene::new("read");
        std::fs::write(
            s.src.join("config.json"),
            br#"{"credsStore":"osxkeychain","credHelpers":{"ghcr.io":"ecr-login"},"auths":{}}"#,
        )
        .unwrap();
        let c = read_user();
        assert!(c.present);
        assert_eq!(c.creds_store.as_deref(), Some("osxkeychain"));
        assert_eq!(c.cred_helpers, vec!["ecr-login".to_string()]);
        assert_eq!(
            c.helper_programs(),
            vec![
                "docker-credential-osxkeychain".to_string(),
                "docker-credential-ecr-login".to_string()
            ]
        );
    }

    #[test]
    fn 没有配置文件时不报错也不瞎猜() {
        let _s = Scene::new("none");
        let c = read_user();
        assert!(!c.present);
        assert!(c.creds_store.is_none());
        assert!(c.helper_programs().is_empty());
    }

    #[test]
    fn 坏_json_按没有助手处理而不是崩() {
        let s = Scene::new("bad");
        std::fs::write(s.src.join("config.json"), b"{not json").unwrap();
        let c = read_user();
        assert!(c.present);
        assert!(c.helper_programs().is_empty());
    }

    /// **不改用户的文件**：跑完之后用户那份 config.json 必须一个字节都没变。
    #[test]
    fn 另起一份配置时不碰用户那份() {
        let s = Scene::new("isolate");
        let original =
            br#"{"credsStore":"osxkeychain","currentContext":"orbstack","psFormat":"table"}"#;
        std::fs::write(s.src.join("config.json"), original).unwrap();
        std::fs::create_dir_all(s.src.join("contexts")).unwrap();
        std::fs::create_dir_all(s.src.join("cli-plugins")).unwrap();

        let out = ensure_isolated().expect("应当能生成");
        assert_eq!(
            std::fs::read(s.src.join("config.json")).unwrap(),
            original.to_vec(),
            "用户那份配置被改动了"
        );

        let made: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.dir.join("config.json")).unwrap())
                .unwrap();
        assert!(made.get("credsStore").is_none(), "credsStore 必须去掉");
        assert_eq!(
            made.get("psFormat").and_then(|x| x.as_str()),
            Some("table"),
            "与凭据无关的键要原样带过来"
        );
        assert!(out.dropped.contains(&"credsStore".to_string()));
        #[cfg(unix)]
        {
            assert!(out.linked.contains(&"contexts".to_string()), "{out:?}");
            assert!(out.linked.contains(&"cli-plugins".to_string()), "{out:?}");
            assert_eq!(
                made.get("currentContext").and_then(|x| x.as_str()),
                Some("orbstack"),
                "contexts 带过来了，currentContext 就该留着"
            );
        }
    }

    /// contexts 带不过来时，`currentContext` 必须跟着去掉 —— 留着会「连不上 daemon」。
    #[test]
    fn 没有_contexts_时不留_currentcontext() {
        let s = Scene::new("noctx");
        std::fs::write(
            s.src.join("config.json"),
            br#"{"credsStore":"desktop","currentContext":"desktop-linux"}"#,
        )
        .unwrap();
        let out = ensure_isolated().unwrap();
        let made: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.dir.join("config.json")).unwrap())
                .unwrap();
        assert!(made.get("currentContext").is_none(), "{made}");
        assert!(out.dropped.contains(&"currentContext".to_string()));
    }

    #[test]
    fn 写出来的配置是_600() {
        let s = Scene::new("perm");
        std::fs::write(s.src.join("config.json"), b"{}").unwrap();
        let out = ensure_isolated().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(out.dir.join("config.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(m & 0o777, 0o600, "{m:o}");
        }
        let _ = out;
    }

    #[test]
    fn 助手名就是_docker_credential_加后缀() {
        assert_eq!(
            helper_program("osxkeychain"),
            "docker-credential-osxkeychain"
        );
        assert_eq!(helper_program(" desktop "), "docker-credential-desktop");
    }
}
