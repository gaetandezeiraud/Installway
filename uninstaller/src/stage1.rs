//! Stage 1: runs from `<install_dir>\uninstall.exe`. Shows confirm dialog,
//! then does the bulk of cleanup (files, shortcuts, registry, empty subdirs).
//! When done, copies itself into `%TEMP%` and spawns Stage 2, then exits so
//! Stage 2 can delete `uninstall.exe` and the install_dir without lock issues.

use crate::cleanup;
use crate::ui::{self, StepCounter, UninstallParams};
use anyhow::{Context, Result};
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

const DETACHED_PROCESS: u32 = 0x00000008;

pub fn run(silent: bool) -> Result<()> {
    let install_dir = cleanup::current_install_dir()?;
    let info = cleanup::read_info(&install_dir)?;
    let manifest = cleanup::read_manifest(&install_dir)?;

    // Uninstall log lives in %TEMP% so it survives the rmdir of install_dir.
    // Stage 2 will append to the same file using our PID as the identifier.
    common::log::init(common::log::log_path_for_stage2(std::process::id()));
    common::log::info(format!(
        "stage1 start: product={} version={} install_dir={} silent={}",
        info.product,
        info.version,
        install_dir.display(),
        silent
    ));

    if silent {
        return run_silent(&install_dir, &info, &manifest);
    }

    let total_steps =
        manifest.files.len() as u64 + 3 /* shortcuts + state + registry */;

    let install_dir_owned = install_dir.clone();
    let info_owned = info.clone();
    let manifest_owned = manifest.clone();
    let tr = ui::tr();

    let params = UninstallParams {
        title: tr.fmt("uninstall.title", &[("product", &info.product)]),
        subtitle: tr.fmt("uninstall.subtitle", &[("version", &info.version)]),
        confirm_text: tr.fmt(
            "uninstall.confirm",
            &[
                ("product", &info.product),
                ("version", &info.version),
                ("path", &info.install_dir),
            ],
        ),
        worker: Box::new(move |progress: Arc<dyn Fn(u64, u64, &str) + Send + Sync>| {
            let counter = StepCounter::new(total_steps, progress);
            let tr = ui::tr();

            // 1. Payload files
            for rel in manifest_owned.files.keys() {
                let p = install_dir_owned.join(rel);
                let _ = fs::remove_file(&p);
                counter.step(&tr.fmt("uninstall.removing", &[("file", rel)]));
            }

            // 2. Shortcuts + file associations
            cleanup::remove_shortcuts(&info_owned.product);
            common::assoc::unregister(&info_owned.product, &info_owned.associations);
            counter.step(&tr.get("uninstall.removing_shortcuts"));

            // 3. State files (manifest, version.json — installer_info.json
            //    kept until just before spawn so stage 2 can still locate things).
            for extra in ["version.json", "installer_manifest.json"] {
                let _ = fs::remove_file(install_dir_owned.join(extra));
            }
            counter.step(&tr.get("uninstall.removing_state"));

            // 4. Empty subdirectories
            cleanup::remove_empty_subdirs(&install_dir_owned);
            counter.report(&tr.get("uninstall.finalizing"));

            // 5. Registry — last so the entry stays visible in Add/Remove Programs
            //    until we know cleanup actually ran.
            cleanup::unregister(&info_owned.registry_key);

            // 6. Spawn Stage 2 (separate temp copy) to finish the job.
            common::log::info("spawning stage 2 to delete install_dir + self");
            if let Err(e) = spawn_stage2(&install_dir_owned, &info_owned.product) {
                common::log::error(format!("stage2 spawn failed: {e:#}"));
                ui::fatal(&tr.fmt("uninstall.spawn_failed", &[("err", &format!("{e:#}"))]));
            }
        }),
        auto_start: false,
    };

    let _ = ui::run(params);
    Ok(())
}

fn run_silent(
    install_dir: &Path,
    info: &common::models::InstallInfo,
    manifest: &common::models::Manifest,
) -> Result<()> {
    let n = cleanup::remove_payload_files(install_dir, manifest);
    common::log::info(format!("removed {} payload files", n));
    cleanup::remove_shortcuts(&info.product);
    common::assoc::unregister(&info.product, &info.associations);
    common::log::info("removed shortcuts + associations");
    let s = cleanup::remove_state_files(install_dir);
    common::log::info(format!("removed {} state files", s));
    cleanup::remove_empty_subdirs(install_dir);
    cleanup::unregister(&info.registry_key);
    common::log::info(format!("unregistered HKCU\\…\\Uninstall\\{}", info.registry_key));
    spawn_stage2(install_dir, &info.product)
}

fn spawn_stage2(install_dir: &Path, product: &str) -> Result<()> {
    let self_exe = std::env::current_exe()?;
    let dest = staged_temp_path()?;
    fs::copy(&self_exe, &dest)
        .with_context(|| format!("copy stage2 to {}", dest.display()))?;

    Command::new(&dest)
        .arg("--stage2")
        .arg(install_dir)
        .arg(product)
        .arg(std::process::id().to_string())
        .creation_flags(DETACHED_PROCESS)
        .spawn()
        .with_context(|| format!("spawn {}", dest.display()))?;
    Ok(())
}

fn staged_temp_path() -> Result<std::path::PathBuf> {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "rustinst-uninstall-{}-{}.exe",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    Ok(p)
}
