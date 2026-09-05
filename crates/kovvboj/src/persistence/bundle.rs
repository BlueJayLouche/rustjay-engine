//! Set bundles — a workspace plus every file it points at, in one archive.
//!
//! A `.kovvbojset` is an uncompressed tar holding the workspace directory and
//! an `assets/` folder with a copy of every shader, video and image the scene
//! references, the recorded paths rewritten to match. Uncompressed on purpose:
//! the bulk of a real set is video, which does not compress, and gzip on a few
//! gigabytes of H.264 is minutes spent for nothing.
//!
//! ponytail: shells out to `tar` (macOS, Linux, and Windows 10+ all ship it)
//! rather than taking a zip dependency. Reach for the `zip` crate if a bundle
//! ever needs random access or per-file progress.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Where packed copies live inside a bundle, and inside an imported workspace.
const ASSETS: &str = "assets";

/// Pack `workspace` and its assets into `out`.
///
/// Save before calling: this reads what is on disk, not what is live.
pub fn export(workspace: &Path, out: &Path) -> anyhow::Result<()> {
    let staging = std::env::temp_dir().join(format!("kovvboj-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    copy_dir(workspace, &staging)?;

    let assets = staging.join(ASSETS);
    std::fs::create_dir_all(&assets)?;
    let base = crate::scene::topology_base();
    let mut packed: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut bytes = 0_u64;
    rewrite_paths(&staging, &mut |path| {
        let src = crate::scene::resolve(path, &base);
        if let Some(done) = packed.get(&src) {
            return Some(done.clone());
        }
        if !src.is_file() {
            log::warn!("[Bundle] skipping missing {}", src.display());
            return None;
        }
        let name = unique_name(&assets, &src);
        match std::fs::copy(&src, assets.join(&name)) {
            Ok(n) => bytes += n,
            Err(e) => {
                log::warn!("[Bundle] could not pack {}: {}", src.display(), e);
                return None;
            }
        }
        let rel = Path::new(ASSETS).join(name);
        packed.insert(src, rel.clone());
        Some(rel)
    })?;

    tar(&[
        OsStr::new("-cf"),
        out.as_os_str(),
        OsStr::new("-C"),
        staging.as_os_str(),
        OsStr::new("."),
    ])?;
    let _ = std::fs::remove_dir_all(&staging);
    log::info!(
        "[Bundle] exported {} ({} asset(s), {} MB) to {}",
        workspace.display(),
        packed.len(),
        bytes / 1_000_000,
        out.display()
    );
    Ok(())
}

/// Unpack `archive` into a new workspace beside it and return the directory.
///
/// The name comes from the archive (`gig.kovvbojset` → `gig/`), numbered if
/// that is taken — an import must never land on top of an existing set.
pub fn import(archive: &Path) -> anyhow::Result<PathBuf> {
    let dir = unique_dir(archive.with_extension(""));
    std::fs::create_dir_all(&dir)?;
    tar(&[
        OsStr::new("-xf"),
        archive.as_os_str(),
        OsStr::new("-C"),
        dir.as_os_str(),
    ])?;

    // Point the packed paths at the copies that just landed. Absolute, so they
    // survive the app being launched from anywhere.
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    let assets = dir.join(ASSETS);
    rewrite_paths(&dir, &mut |path| {
        let name = path.strip_prefix(ASSETS).ok()?;
        Some(assets.join(name))
    })?;
    log::info!("[Bundle] imported {} to {}", archive.display(), dir.display());
    Ok(dir)
}

/// Run `f` over every asset path recorded anywhere in a workspace directory,
/// replacing the ones it answers for.
///
/// Works on the JSON rather than the typed scene: every path in a scene, saved
/// layer, chain or group sits under a `"path"` key, so one walk covers all four
/// and keeps covering them as those structs grow.
fn rewrite_paths(
    dir: &Path,
    f: &mut impl FnMut(&Path) -> Option<PathBuf>,
) -> anyhow::Result<()> {
    let mut files = vec![dir.join("scene.json")];
    for sub in ["layers", "chains", "groups"] {
        if let Ok(entries) = std::fs::read_dir(dir.join(sub)) {
            files.extend(
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension() == Some(OsStr::new("json"))),
            );
        }
    }

    for file in files.iter().filter(|p| p.is_file()) {
        let mut json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file)?)?;
        walk(&mut json, f);
        std::fs::write(file, serde_json::to_string_pretty(&json)?)?;
    }
    Ok(())
}

fn walk(value: &mut serde_json::Value, f: &mut impl FnMut(&Path) -> Option<PathBuf>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key == "path"
                    && let Some(text) = child.as_str()
                    && let Some(new) = f(Path::new(text))
                {
                    *child = serde_json::Value::String(new.to_string_lossy().into_owned());
                    continue;
                }
                walk(child, f);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(|v| walk(v, f)),
        _ => {}
    }
}

/// A free name for `src` in `assets`, so two different shaders both called
/// `blur.fs` do not become one.
fn unique_name(assets: &Path, src: &Path) -> PathBuf {
    let name = src.file_name().unwrap_or(OsStr::new("asset"));
    let mut candidate = PathBuf::from(name);
    let mut n = 2;
    while assets.join(&candidate).exists() {
        candidate = PathBuf::from(format!("{n}_{}", name.to_string_lossy()));
        n += 1;
    }
    candidate
}

fn unique_dir(dir: PathBuf) -> PathBuf {
    if !dir.exists() {
        return dir;
    }
    let name = dir.file_name().unwrap_or(OsStr::new("set")).to_os_string();
    (2..)
        .map(|n| dir.with_file_name(format!("{} {n}", name.to_string_lossy())))
        .find(|p| !p.exists())
        .unwrap_or(dir)
}

/// Copy a directory recursively, leaving `assets/` behind — export repacks it
/// from the scene, so carrying the old copies across would duplicate every file
/// each time a bundle was re-exported.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        if entry.file_name() == OsStr::new(ASSETS) {
            continue;
        }
        let dst = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dst)?;
        } else {
            std::fs::copy(entry.path(), dst)?;
        }
    }
    Ok(())
}

fn tar(args: &[&OsStr]) -> anyhow::Result<()> {
    let status = std::process::Command::new("tar").args(args).status()?;
    anyhow::ensure!(status.success(), "tar exited with {status}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of a bundle: paths recorded anywhere in the workspace
    /// come out pointing at the packed copy, at whatever nesting they sit.
    #[test]
    fn every_recorded_path_is_rewritten() {
        let dir = std::env::temp_dir().join(format!("kovvboj-bundle-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("layers")).unwrap();
        std::fs::write(
            dir.join("scene.json"),
            r#"{"topology":{"layers":[{"source":{"path":"clip.mov"},"fx":[{"path":"blur.fs"}]}]}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("layers").join("a.json"), r#"{"layer":{"source":{"path":"clip.mov"}}}"#)
            .unwrap();

        rewrite_paths(&dir, &mut |p| Some(Path::new(ASSETS).join(p))).unwrap();

        let scene = std::fs::read_to_string(dir.join("scene.json")).unwrap();
        assert!(scene.contains("assets/clip.mov"), "{scene}");
        assert!(scene.contains("assets/blur.fs"), "{scene}");
        let layer = std::fs::read_to_string(dir.join("layers").join("a.json")).unwrap();
        assert!(layer.contains("assets/clip.mov"), "{layer}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The round trip that matters: a set exported on one machine and imported
    /// on another finds its clip, without either path being valid on both.
    #[test]
    fn a_bundle_carries_its_assets() {
        let root = std::env::temp_dir().join(format!("kovvboj-roundtrip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ws = root.join("live");
        std::fs::create_dir_all(&ws).unwrap();
        let clip = root.join("clip.mov");
        std::fs::write(&clip, b"not really a movie").unwrap();
        std::fs::write(
            ws.join("scene.json"),
            format!(
                r#"{{"topology":{{"layers":[{{"source":{{"path":{:?}}}}}]}}}}"#,
                clip.to_string_lossy()
            ),
        )
        .unwrap();

        let archive = root.join("gig.kovvbojset");
        export(&ws, &archive).unwrap();

        // The source is gone, as it would be on someone else's machine.
        std::fs::remove_file(&clip).unwrap();
        let imported = import(&archive).unwrap();

        let scene = std::fs::read_to_string(imported.join("scene.json")).unwrap();
        let packed = imported.join(ASSETS).join("clip.mov");
        assert!(packed.is_file(), "asset not unpacked: {}", packed.display());
        assert!(
            scene.contains(&*packed.to_string_lossy()),
            "scene still points elsewhere: {scene}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn same_name_assets_do_not_collide() {
        let assets = std::env::temp_dir().join(format!("kovvboj-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&assets);
        std::fs::create_dir_all(&assets).unwrap();
        let first = unique_name(&assets, Path::new("/a/blur.fs"));
        std::fs::write(assets.join(&first), "").unwrap();
        let second = unique_name(&assets, Path::new("/b/blur.fs"));
        assert_ne!(first, second);
        let _ = std::fs::remove_dir_all(&assets);
    }
}
