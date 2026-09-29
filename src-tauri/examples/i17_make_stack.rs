//! **临时脚手架**（只为 I17 的现场造景，跑完就删，不进仓库）。
//!
//! 这台香港开发机上原来没有 `hunter` 项目的那套测试栈，而 `--headless` 安装
//! 必须先过一道网关 key 校验（本机没有可用的 hunter key，实测报 `E_KEY_INVALID`）。
//! 造景要的不是「安装流程本身」，而是「一台装好了的机器」：
//! `~/.hunter/launcher.toml` + `~/.hunter/app/{docker-compose.yml,.env,覆盖文件}`。
//!
//! 所以这里**不手抄模板**，而是调用启动器自己的那几个渲染函数
//! （`config::fetch_compose` / `write_override_local` / `write_env` /
//! `LauncherConfig::save`）把配置写到 `HUNTER_HOME` 下 —— 与真安装写出来的
//! 是同一份东西，不会因为抄错模板而把造景的结论弄脏。
//!
//! 用法：`I17_TAG=1.2.0 I17_REGISTRY=ghcr cargo run --example i17_make_stack`

use std::time::Duration;

use hunter_launcher_lib::{config, gateway, paths, registry};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tag = std::env::var("I17_TAG").unwrap_or_else(|_| "1.2.0".to_string());
    let reg_id = std::env::var("I17_REGISTRY").unwrap_or_else(|_| "ghcr".to_string());
    let key = std::env::var("I17_KEY").unwrap_or_default();

    paths::ensure_dirs()?;
    println!("工作目录 {}", paths::root().display());

    // 端口：本机 3100 / 8100 / 3921 / 5442 / 6479 都被 hca-* 与 xinghe-* 占着，
    // 用它们各自 +1 的那一组（与 I6 报告里那台测试机写出来的一样）
    let ports = config::Ports {
        web: 3101,
        api: 8101,
        opencode: 3922,
        postgres: 5443,
        redis: 6480,
    };

    let cand = registry::by_id(&reg_id).ok_or("不认识的镜像源 id")?;

    // ① compose（拿不到就退回内置的 1.2.0 副本，与真安装同一套兜底）
    let (yml, src, note) = config::fetch_compose(&tag, Duration::from_secs(30));
    std::fs::write(paths::compose_file(), &yml)?;
    println!(
        "① {} ← {:?}（{} 字节）{}",
        paths::compose_file().display(),
        src,
        yml.len(),
        note.map(|n| format!("· {n}")).unwrap_or_default()
    );

    // ② 覆盖文件（六个服务一律 127.0.0.1）
    config::write_override_local(&ports, &cand.base_prefix)?;
    println!("② {} 已写", paths::override_file().display());

    // ③ .env
    let input = config::EnvInput {
        tag: &tag,
        registry_prefix: &cand.prefix,
        ports: &ports,
        hunter_key: &key,
        model_mode: "gateway",
        llm_base_url: gateway::LLM_BASE_URL,
        llm_model: gateway::DEFAULT_MODEL,
        llm_api_key: "",
        schema_sanitize: false,
    };
    config::write_env(&input)?;
    println!("③ {} 已写（权限 600）", paths::env_file().display());

    // ④ launcher.toml
    let mut cfg = config::LauncherConfig::load();
    cfg.hunter.tag = tag.clone();
    cfg.hunter.registry_id = cand.id.to_string();
    cfg.hunter.registry_prefix = cand.prefix.to_string();
    cfg.hunter.base_prefix = cand.base_prefix.to_string();
    cfg.hunter.ports = ports.clone();
    cfg.mark_installed(false);
    cfg.save()?;
    println!(
        "④ {} 已写（hunter.tag={} · install.done={}）",
        paths::launcher_toml().display(),
        cfg.hunter.tag,
        cfg.install.done
    );

    println!(
        "\n下一步：docker compose -p hunter --project-directory {} \\",
        paths::app_dir().display()
    );
    println!("          -f docker-compose.yml -f docker-compose.launcher.yml up -d");
    Ok(())
}
