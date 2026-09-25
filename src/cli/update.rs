// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `cig update`: check for and install a newer release.

use super::{exit, Ctx};
use cigscript::config::Config;
use cigscript::update::{self, Version};
use std::cmp::Ordering;

pub fn update(ctx: &Ctx, check_only: bool, allow_downgrade: bool, to: Option<String>) -> i32 {
    let cfg = Config::load();
    let repo = cfg.get("issues_repo");
    let prerelease = cfg.get("update_channel") == "prerelease";
    let current = Version::current();

    let release = match &to {
        Some(tag) => match update::find(&repo, tag) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{} {e}", ctx.red("error[E806 usage]:"));
                return exit::USAGE;
            }
        },
        None => match update::latest(&repo, prerelease) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{} {e}", ctx.red("error[E806 usage]:"));
                return exit::USAGE;
            }
        },
    };
    let Some(target) = release.version() else {
        eprintln!(
            "{} release {} has no parsable version",
            ctx.red("error[E806 usage]:"),
            release.tag
        );
        return exit::USAGE;
    };
    let _ = update::write_cache(&release);
    let ordering = target.cmp(&current);

    if ctx.json {
        outln!(
            "{}",
            serde_json::json!({
                "current": current.to_string(),
                "latest": target.to_string(),
                "tag": release.tag,
                "url": release.url,
                "newer": ordering == Ordering::Greater,
                "checked_only": check_only,
            })
        );
        if check_only {
            return exit::OK;
        }
    } else {
        match ordering {
            Ordering::Greater => eprintln!(
                "{} {current} -> {target}  {}",
                ctx.bold("update available:"),
                ctx.dim(&release.url)
            ),
            Ordering::Equal => eprintln!(
                "{} {current} is the newest release",
                ctx.green("up to date:")
            ),
            Ordering::Less => eprintln!(
                "{} you run {current}, the newest release is {target}",
                ctx.yellow("ahead:")
            ),
        }
        if check_only {
            return exit::OK;
        }
    }

    if ordering == Ordering::Equal {
        return exit::OK;
    }
    if ordering == Ordering::Less && !allow_downgrade {
        eprintln!("refusing to downgrade; pass --allow-downgrade if you really want {target}");
        return exit::USAGE;
    }

    let name = update::asset_name(&target);
    let Some(asset) = release.asset(&name) else {
        eprintln!(
            "{} release {} has no asset named {name} for this platform; build from source with `cargo install --git https://github.com/{repo} --tag {}`",
            ctx.red("error[E806 usage]:"),
            release.tag,
            release.tag
        );
        return exit::USAGE;
    };
    let Some(sidecar) = release.asset(&format!("{name}.sha256")) else {
        eprintln!("{} release {} ships no checksum for {name}; refusing to install an unverifiable binary", ctx.red("error[E806 usage]:"), release.tag);
        return exit::USAGE;
    };

    let tmp = cigscript::burn::runs::home_dir().join("tmp");
    if let Err(e) = std::fs::create_dir_all(&tmp) {
        eprintln!(
            "{} cannot create {}: {e}",
            ctx.red("error[E802 usage]:"),
            tmp.display()
        );
        return exit::USAGE;
    }
    let bin = tmp.join(&name);
    let sum = tmp.join(format!("{name}.sha256"));
    eprintln!("downloading {name} ...");
    if let Err(e) =
        update::download(&asset.url, &bin).and_then(|()| update::download(&sidecar.url, &sum))
    {
        eprintln!("{} {e}", ctx.red("error[E806 usage]:"));
        let _ = std::fs::remove_file(&bin);
        return exit::USAGE;
    }
    let sidecar_text = std::fs::read_to_string(&sum).unwrap_or_default();
    if let Err(e) = update::verify_sha256(&bin, &sidecar_text) {
        eprintln!("{} {e}", ctx.red("error[E806 usage]:"));
        let _ = std::fs::remove_file(&bin);
        return exit::USAGE;
    }
    eprintln!(
        "{} sha256 matches the published checksum (integrity, not authorship: see SECURITY.md)",
        ctx.green("verified:")
    );
    match update::install(&bin) {
        Ok(path) => {
            let _ = std::fs::remove_file(&bin);
            let _ = std::fs::remove_file(&sum);
            eprintln!(
                "{} {} is now {target} ({})",
                ctx.green("updated:"),
                path.display(),
                release.tag
            );
            exit::OK
        }
        Err(e) => {
            eprintln!("{} {e}", ctx.red("error[E806 usage]:"));
            eprintln!(
                "the verified binary is at {}; copy it into place yourself",
                bin.display()
            );
            exit::USAGE
        }
    }
}
