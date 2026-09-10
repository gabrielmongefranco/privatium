// This file is part of Privatium
// crates/privatium/build.rs
// Author(s): Gabriel Mongefranco
// Created: 2026-09-08
// Last Modified: 2026-09-08
// Summary: The two project facts Cargo has no field for — the copyright holder and year, and the
//          author's site — and, on Windows, the VS_VERSION_INFO resource that fills the
//          Details tab of the file properties dialog. Everything else both the resource and
//          `privatium --version` (spec/cli.md §1) print comes from [workspace.package] through
//          the CARGO_PKG_* variables, so the manifest stays the one place a name, a licence or
//          a URL is written down.
// Notes: See README file for documentation and full license information.
//
// Copyright © 2026 Gabriel Mongefranco
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along
// with this program. If not, see <https://www.gnu.org/licenses/>.

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

/// The copyright holder and year, and the author's site. Cargo carries no field for
/// either, and both `--version` and the Windows resource need them, so they are written
/// once here and reach the program as `PV_COPYRIGHT_*` and `PV_AUTHOR_URL`. The notice
/// itself is the one in `NOTICE` and `README.md`.
const COPYRIGHT_YEAR: &str = "2026";
const COPYRIGHT_HOLDER: &str = "Gabriel Mongefranco";
const AUTHOR_URL: &str = "https://gabriel.mongefranco.com";

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    // Naming any file in a rerun-if rule narrows this script to the files it names, and
    // everything it writes into the resource comes from the two manifests: this package's
    // description and name, and [workspace.package]'s version, licence, authors and
    // repository. Without these two lines an edited manifest reaches `--version`, which is
    // recompiled, and not the resource, which is not — and a shipped binary would say two
    // different things. `rerun-if-env-changed` does not cover it: Cargo sets the CARGO_PKG_*
    // variables itself and does not track them for a build script.
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rustc-env=PV_COPYRIGHT_YEAR={COPYRIGHT_YEAR}");
    println!("cargo:rustc-env=PV_COPYRIGHT_HOLDER={COPYRIGHT_HOLDER}");
    println!("cargo:rustc-env=PV_AUTHOR_URL={AUTHOR_URL}");

    // A Mach-O binary and an ELF binary have nowhere to put any of this: neither format
    // has a section a file manager reads, and inventing one would mean a linker script for
    // a string nothing displays. `--version` is the answer on both (spec/cli.md §1).
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is not set")?);
    let script = out.join("version.rc");
    fs::write(&script, version_rc()?)?;
    let path = script.to_str().ok_or("the build directory is not UTF-8")?;
    // `manifest_required`: the resource carries the version information the properties
    // dialog reads, so a build that silently dropped it would ship a blank Details tab.
    embed_resource::compile(path, embed_resource::NONE).manifest_required()?;
    Ok(())
}

/// The `VS_VERSION_INFO` resource script, in the fields the Details tab shows. Every
/// value comes from the manifest or from the constants above.
fn version_rc() -> Result<String, Box<dyn Error>> {
    let field = |name: &str| -> Result<String, Box<dyn Error>> {
        Ok(env::var(name).map_err(|_| format!("{name} is not set"))?)
    };
    let version = field("CARGO_PKG_VERSION")?;
    let quad = format!(
        "{},{},{},0",
        field("CARGO_PKG_VERSION_MAJOR")?,
        field("CARGO_PKG_VERSION_MINOR")?,
        field("CARGO_PKG_VERSION_PATCH")?
    );
    let copyright = format!("Copyright (c) {COPYRIGHT_YEAR} {COPYRIGHT_HOLDER}. Licensed under ")
        + &field("CARGO_PKG_LICENSE")?
        + ".";
    let values = [
        ("CompanyName", field("CARGO_PKG_AUTHORS")?),
        ("FileDescription", field("CARGO_PKG_DESCRIPTION")?),
        ("FileVersion", version.clone()),
        ("InternalName", field("CARGO_PKG_NAME")?),
        ("LegalCopyright", copyright),
        (
            "OriginalFilename",
            format!("{}.exe", field("CARGO_PKG_NAME")?),
        ),
        ("ProductName", "Privatium".to_owned()),
        ("ProductVersion", version),
        (
            "Comments",
            format!(
                "Project: {} — Author: {AUTHOR_URL}",
                field("CARGO_PKG_REPOSITORY")?
            ),
        ),
    ];
    // The resource is named by the number 1, not by the `VS_VERSION_INFO` identifier: that
    // identifier is defined as 1 in the SDK's winver.h, and a script that does not include
    // the header would give the resource a *name* instead of that ID — which compiles, and
    // then Windows finds no version information at all.
    let mut out = String::from("// Generated by build.rs; edit that, not this.\n1 VERSIONINFO\n");
    // 0x40004 is VOS_NT_WINDOWS32 and 0x1 is VFT_APP, written as numbers so the script
    // needs no header from the Windows SDK to compile.
    out.push_str(&format!(
        "FILEVERSION {quad}\nPRODUCTVERSION {quad}\nFILEFLAGSMASK 0x3fL\nFILEFLAGS 0x0L\n\
         FILEOS 0x40004L\nFILETYPE 0x1L\nFILESUBTYPE 0x0L\nBEGIN\n  \
         BLOCK \"StringFileInfo\"\n  BEGIN\n    BLOCK \"040904b0\"\n    BEGIN\n"
    ));
    for (name, value) in values {
        out.push_str(&format!(
            "      VALUE \"{name}\", \"{}\"\n",
            rc_string(&value)
        ));
    }
    out.push_str(
        "    END\n  END\n  BLOCK \"VarFileInfo\"\n  BEGIN\n    \
         VALUE \"Translation\", 0x409, 1200\n  END\nEND\n",
    );
    Ok(out)
}

/// One string value for the resource script. A resource compiler reads the script in the
/// system code page unless it is told otherwise, and the toolchains disagree on how to
/// tell it, so the text is folded to ASCII here: the punctuation this project uses in
/// prose has a plain spelling, and anything else becomes `?` rather than a mojibake byte
/// nobody can read in a properties dialog.
fn rc_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\"\""),
            '\\' => out.push_str("\\\\"),
            '—' | '–' => out.push('-'),
            '©' => out.push_str("(c)"),
            '™' => out.push_str("(TM)"),
            '‘' | '’' => out.push('\''),
            '“' | '”' => out.push('"'),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            _ => out.push('?'),
        }
    }
    out
}
