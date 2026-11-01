//! Safe handling of files that arrive from other people: spotting files that can run code, and marking saved files
//! as coming from the internet, so the OS checks them when they are opened.
//!
//! This is not malware detection. It says what kind of file something is, never that a file is safe. A harmful PDF, a
//! program inside a zip and a macro hidden in an unusual place all pass. See `docs/design/08-files.md`.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use serde::Serialize;

use crate::name::sanitize_name;

/// Bytes read from the start of a file for its magic numbers.
const HEAD_BYTES: u64 = 8192;
/// A zip's list of names larger than this is not read, since a sender controls its size.
const MAX_ZIP_DIRECTORY: u64 = 64 * 1024 * 1024;
/// An OLE file larger than this is not parsed: the parser's tables grow with the file, and documents stay far smaller.
const MAX_OLE_BYTES: u64 = 256 * 1024 * 1024;

/// Why a file is not opened from the app. Sent to the UI, which words each kind for the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Danger {
    Program,
    Installer,
    Script,
    Shortcut,
    Macro,
    DiskImage,
    /// The content could not be read or made sense of, so it gets the same care as a program.
    Unchecked,
}

/// What a file's name and claimed MIME type say. Both are the sender's words, so a `None` here means little: once the
/// file is on disk, [`danger_of_path`] also reads its content.
pub fn danger_of_name(name: &str, mime: &str) -> Option<Danger> {
    danger_of_ext(&sanitize_name(name).to_ascii_lowercase()).or_else(|| danger_of_mime(&mime.to_ascii_lowercase()))
}

fn danger_of_ext(name: &str) -> Option<Danger> {
    if name == "desktop.ini" {
        return Some(Danger::Shortcut);
    }
    let (_, ext) = name.rsplit_once('.')?;
    Some(match ext {
        "exe" | "com" | "scr" | "pif" | "cpl" | "dll" | "sys" | "drv" | "ocx" | "msc" | "gadget" | "jar" | "class" | "dex" | "apk"
        | "ipa" | "app" | "run" | "appimage" => Danger::Program,
        "msi" | "msp" | "mst" | "msix" | "msixbundle" | "appx" | "appxbundle" | "application" | "appref-ms" | "deb" | "rpm" | "pkg"
        | "mpkg" => Danger::Installer,
        "bat" | "cmd" | "ps1" | "ps2" | "psm1" | "psd1" | "ps1xml" | "vbs" | "vbe" | "js" | "jse" | "mjs" | "wsf" | "wsh" | "wsc" | "hta"
        | "sct" | "sh" | "bash" | "zsh" | "fish" | "ksh" | "csh" | "command" | "tool" | "py" | "pyw" | "pyc" | "pl" | "rb" | "php"
        | "lua" | "scpt" | "applescript" | "reg" | "inf" | "chm" => Danger::Script,
        "lnk" | "url" | "scf" | "desktop" | "webloc" | "website" | "library-ms" | "settingcontent-ms" => Danger::Shortcut,
        "docm" | "dotm" | "xlsm" | "xltm" | "xlam" | "xlsb" | "pptm" | "potm" | "ppsm" | "ppam" | "sldm" => Danger::Macro,
        "dmg" | "iso" | "img" | "vhd" | "vhdx" => Danger::DiskImage,
        _ => return None,
    })
}

fn danger_of_mime(mime: &str) -> Option<Danger> {
    if mime.contains("macroenabled") {
        return Some(Danger::Macro);
    }
    Some(match mime {
        "application/x-msdownload" | "application/x-dosexec" | "application/vnd.microsoft.portable-executable" | "application/x-executable"
        | "application/x-elf" | "application/x-mach-binary" | "application/java-archive" | "application/vnd.android.package-archive" => {
            Danger::Program
        }
        "application/x-msi" | "application/x-ms-installer" | "application/x-deb" | "application/vnd.debian.binary-package"
        | "application/x-rpm" | "application/vnd.apple.installer+xml" => Danger::Installer,
        "application/x-sh" | "application/x-shellscript" | "text/x-shellscript" | "application/x-csh" | "application/x-bat"
        | "application/x-powershell" | "application/hta" | "application/javascript" | "text/javascript" | "text/vbscript" => Danger::Script,
        "application/x-ms-shortcut" => Danger::Shortcut,
        "application/x-apple-diskimage" | "application/x-iso9660-image" => Danger::DiskImage,
        _ => return None,
    })
}

/// What a file on disk is, from its name and its content. A file that is gone gives `None`. One that cannot be read
/// gives [`Danger::Unchecked`], and so does a parser that panics on a crafted file.
pub fn danger_of_path(path: &Path) -> Option<Danger> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if let Some(danger) = danger_of_ext(&sanitize_name(&name).to_ascii_lowercase()) {
        return Some(danger);
    }
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(_) => return Some(Danger::Unchecked),
    };
    match catch_unwind(AssertUnwindSafe(|| danger_of_content(&mut file))) {
        Ok(Ok(danger)) => danger,
        Ok(Err(_)) | Err(_) => Some(Danger::Unchecked),
    }
}

/// What a file's bytes say, whatever it is called. An error means a container that claims to be a zip or an OLE file
/// and cannot be walked, which says nothing about what is inside.
pub fn danger_of_content<R: Read + Seek>(file: &mut R) -> io::Result<Option<Danger>> {
    let len = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;
    let mut head = Vec::new();
    file.by_ref().take(HEAD_BYTES).read_to_end(&mut head)?;
    if let Some(danger) = danger_of_head(&head) {
        return Ok(Some(danger));
    }
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        return zip_danger(file, len);
    }
    if head.starts_with(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") {
        return ole_danger(file, len);
    }
    disk_image_at_fixed_places(file, len)
}

fn danger_of_head(head: &[u8]) -> Option<Danger> {
    use infer::{app, archive};
    let without_bom = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    if without_bom.starts_with(b"#!") {
        return Some(Danger::Script);
    }
    if app::is_exe(head)
        || head.starts_with(b"\x7FELF")
        || app::is_elf(head)
        || app::is_mach(head)
        || app::is_java(head)
        || app::is_dex(head)
        || app::is_dey(head)
    {
        return Some(Danger::Program);
    }
    if archive::is_deb(head) || archive::is_rpm(head) || head.starts_with(b"xar!") {
        return Some(Danger::Installer);
    }
    if head.starts_with(b"\x4C\x00\x00\x00\x01\x14\x02\x00") {
        return Some(Danger::Shortcut);
    }
    if head.starts_with(b"conectix") || head.starts_with(b"vhdxfile") {
        return Some(Danger::DiskImage);
    }
    let text = leading_text(head);
    if text.starts_with("[internetshortcut]") || text.starts_with("[desktop entry]") {
        return Some(Danger::Shortcut);
    }
    if text.starts_with("windows registry editor") || text.starts_with("regedit4") || text.starts_with("@echo off") {
        return Some(Danger::Script);
    }
    None
}

/// The first characters as lowercase ASCII. `.reg` files are UTF-16 as Windows writes them.
fn leading_text(head: &[u8]) -> String {
    let bytes: Vec<u8> = match head.strip_prefix(b"\xFF\xFE") {
        Some(wide) => wide.iter().step_by(2).copied().take(32).collect(),
        None => head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head).iter().copied().take(32).collect(),
    };
    String::from_utf8_lossy(&bytes).to_ascii_lowercase()
}

/// Disk images whose signatures do not sit at the start: ISO 9660 at 0x8001, DMG and fixed VHD in the last 512 bytes.
fn disk_image_at_fixed_places<R: Read + Seek>(file: &mut R, len: u64) -> io::Result<Option<Danger>> {
    if len >= 0x8006 {
        let mut iso = [0u8; 5];
        file.seek(SeekFrom::Start(0x8001))?;
        file.read_exact(&mut iso)?;
        if &iso == b"CD001" {
            return Ok(Some(Danger::DiskImage));
        }
    }
    if len >= 512 {
        let mut tail = [0u8; 8];
        file.seek(SeekFrom::Start(len - 512))?;
        file.read_exact(&mut tail)?;
        if tail.starts_with(b"koly") || &tail == b"conectix" {
            return Ok(Some(Danger::DiskImage));
        }
    }
    Ok(None)
}

fn bad_container(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

fn le16(b: &[u8], at: usize) -> io::Result<u64> {
    let s = b.get(at..at + 2).ok_or_else(|| bad_container("short zip record"))?;
    Ok(u16::from_le_bytes([s[0], s[1]]) as u64)
}

fn le32(b: &[u8], at: usize) -> io::Result<u64> {
    let s = b.get(at..at + 4).ok_or_else(|| bad_container("short zip record"))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64)
}

fn le64(b: &[u8], at: usize) -> io::Result<u64> {
    Ok(le32(b, at)? | (le32(b, at + 4)? << 32))
}

/// A zip's central directory, read by hand so a sender cannot make the check use more than [`MAX_ZIP_DIRECTORY`].
fn zip_directory<R: Read + Seek>(file: &mut R, len: u64) -> io::Result<Vec<u8>> {
    let tail_len = len.min(22 + 65_535);
    file.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0u8; tail_len as usize];
    file.read_exact(&mut tail)?;
    let end = tail
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .ok_or_else(|| bad_container("zip without an end record"))?;
    let (mut size, mut offset) = (le32(&tail, end + 12)?, le32(&tail, end + 16)?);
    if size == u32::MAX as u64 || offset == u32::MAX as u64 {
        let locator = tail.get(end.saturating_sub(20)..end).filter(|l| l.starts_with(b"PK\x06\x07"));
        let locator = locator.ok_or_else(|| bad_container("zip64 without a locator"))?;
        let mut record = [0u8; 56];
        file.seek(SeekFrom::Start(le64(locator, 8)?))?;
        file.read_exact(&mut record)?;
        if !record.starts_with(b"PK\x06\x06") {
            return Err(bad_container("zip64 without an end record"));
        }
        (size, offset) = (le64(&record, 40)?, le64(&record, 48)?);
    }
    if size > MAX_ZIP_DIRECTORY || offset.checked_add(size).is_none_or(|e| e > len) {
        return Err(bad_container("zip directory out of bounds"));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut directory = vec![0u8; size as usize];
    file.read_exact(&mut directory)?;
    Ok(directory)
}

/// A zip is only as dangerous as what it really is: an Office file with macros, a Java or Android program, or a Mac or
/// iOS app. Other files inside a plain zip are not judged.
fn zip_danger<R: Read + Seek>(file: &mut R, len: u64) -> io::Result<Option<Danger>> {
    let directory = zip_directory(file, len)?;
    let mut at = 0;
    while at < directory.len() {
        if directory.get(at..at + 4) != Some(b"PK\x01\x02") {
            return Err(bad_container("bad zip directory entry"));
        }
        let name_len = le16(&directory, at + 28)? as usize;
        let rest = (le16(&directory, at + 30)? + le16(&directory, at + 32)?) as usize;
        let name = directory.get(at + 46..at + 46 + name_len).ok_or_else(|| bad_container("short zip name"))?;
        let name = String::from_utf8_lossy(name).replace('\\', "/").to_ascii_lowercase();
        if let Some(danger) = danger_of_zip_entry(&name) {
            return Ok(Some(danger));
        }
        at += 46 + name_len + rest;
    }
    Ok(None)
}

fn danger_of_zip_entry(name: &str) -> Option<Danger> {
    if name.ends_with("vbaproject.bin") || name.contains("/macrosheets/") {
        return Some(Danger::Macro);
    }
    let app = name == "meta-inf/manifest.mf"
        || name == "androidmanifest.xml"
        || (name.starts_with("payload/") && name.contains(".app/"))
        || name.contains(".app/contents/macos/");
    app.then_some(Danger::Program)
}

/// An OLE file is a Windows installer or an old Office document. The second is dangerous when it holds a macro project.
fn ole_danger<R: Read + Seek>(file: &mut R, len: u64) -> io::Result<Option<Danger>> {
    if len > MAX_OLE_BYTES {
        return Err(bad_container("OLE file too large to check"));
    }
    file.seek(SeekFrom::Start(0))?;
    let ole = cfb::CompoundFile::open(file)?;
    let clsid = ole.root_entry().clsid().to_string();
    if clsid.starts_with("000c108") && clsid.ends_with("-0000-0000-c000-000000000046") {
        return Ok(Some(Danger::Installer));
    }
    let has_macros = ole.walk().any(|entry| {
        let name = entry.name();
        ["_VBA_PROJECT", "_VBA_PROJECT_CUR", "VBA", "Macros"].iter().any(|m| name.eq_ignore_ascii_case(m))
    });
    Ok(has_macros.then_some(Danger::Macro))
}

/// Marks a file saved from another person as coming from the internet, so SmartScreen and Gatekeeper check it when it
/// is opened. Elsewhere there is nothing to mark.
#[cfg(windows)]
pub fn mark_received(path: &Path) -> io::Result<()> {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    std::fs::write(std::path::PathBuf::from(stream), "[ZoneTransfer]\r\nZoneId=3\r\n")
}

/// See the Windows version. The flags mean "downloaded by an app, not yet approved".
#[cfg(target_os = "macos")]
pub fn mark_received(path: &Path) -> io::Result<()> {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    xattr::set(path, "com.apple.quarantine", format!("0081;{secs:08x};Dissent;").as_bytes())
}

/// See the Windows version.
#[cfg(not(any(windows, target_os = "macos")))]
pub fn mark_received(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Marks a file that was just saved. A folder that cannot hold the mark, such as one on FAT, keeps the file.
pub(crate) fn mark_or_log(path: &Path) {
    if let Err(e) = mark_received(path) {
        tracing::warn!(error = %e, path = %path.display(), "could not mark a received file as coming from the internet");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn content(bytes: &[u8]) -> Option<Danger> {
        danger_of_content(&mut Cursor::new(bytes)).expect("readable")
    }

    fn pe() -> Vec<u8> {
        let mut b = vec![0u8; 256];
        b[..2].copy_from_slice(b"MZ");
        b[0x3C] = 0x80;
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        b
    }

    /// A zip of empty stored files.
    fn zip(names: &[&str]) -> Vec<u8> {
        let (mut out, mut dir) = (Vec::new(), Vec::new());
        for name in names {
            let offset = out.len() as u32;
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&[0u8; 22]);
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0u8; 2]);
            out.extend_from_slice(name.as_bytes());
            dir.extend_from_slice(b"PK\x01\x02");
            dir.extend_from_slice(&[0u8; 24]);
            dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
            dir.extend_from_slice(&[0u8; 12]);
            dir.extend_from_slice(&offset.to_le_bytes());
            dir.extend_from_slice(name.as_bytes());
        }
        let at = out.len() as u32;
        out.extend_from_slice(&dir);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0u8; 4]);
        out.extend_from_slice(&(names.len() as u16).to_le_bytes());
        out.extend_from_slice(&(names.len() as u16).to_le_bytes());
        out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&[0u8; 2]);
        out
    }

    /// An OLE file with these streams, each path's folders created first.
    fn ole(streams: &[&str]) -> Vec<u8> {
        let mut file = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        for stream in streams {
            if let Some((folder, _)) = stream.rsplit_once('/') {
                file.create_storage_all(format!("/{folder}")).unwrap();
            }
            file.create_stream(format!("/{stream}")).unwrap().write_all(b"x").unwrap();
        }
        file.flush().unwrap();
        file.into_inner().into_inner()
    }

    fn scratch(tag: &str, name: &str, bytes: &[u8]) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("dissent-received-{tag}-{}-{:x}", std::process::id(), rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    #[test]
    fn names_are_judged_by_their_real_extension_in_any_case() {
        assert_eq!(danger_of_name("setup.EXE", ""), Some(Danger::Program));
        assert_eq!(danger_of_name("invoice.pdf.exe", "application/pdf"), Some(Danger::Program));
        assert_eq!(danger_of_name("run.ps1", ""), Some(Danger::Script));
        assert_eq!(danger_of_name("a.msi", ""), Some(Danger::Installer));
        assert_eq!(danger_of_name("Home.lnk", ""), Some(Danger::Shortcut));
        assert_eq!(danger_of_name("q3.xlsm", ""), Some(Danger::Macro));
        assert_eq!(danger_of_name("disk.dmg", ""), Some(Danger::DiskImage));
        assert_eq!(danger_of_name("notes.pdf", "application/pdf"), None);
        assert_eq!(danger_of_name("photo.png", "image/png"), None);
        assert_eq!(danger_of_name("no-extension", ""), None);
    }

    #[test]
    fn tricks_in_a_name_do_not_hide_its_extension() {
        assert_eq!(danger_of_name("evil.exe. . ", ""), Some(Danger::Program), "Windows drops trailing dots and spaces");
        assert_eq!(danger_of_name("gpj\u{202E}.exe", ""), Some(Danger::Program));
        assert_eq!(danger_of_name("a/b\\evil.bat", ""), Some(Danger::Script));
        // Sanitizing cuts a long name, which can leave a harmful extension at the end.
        let long = format!("{}.exe.pdf", "a".repeat(124));
        assert_eq!(danger_of_name(&long, "application/pdf"), Some(Danger::Program));
    }

    #[test]
    fn a_claimed_mime_type_counts_against_a_harmless_name() {
        assert_eq!(danger_of_name("cat.txt", "application/x-msdownload"), Some(Danger::Program));
        assert_eq!(danger_of_name("cat.txt", "application/vnd.ms-excel.sheet.macroEnabled.12"), Some(Danger::Macro));
    }

    #[test]
    fn a_renamed_program_is_caught_by_its_content() {
        assert_eq!(content(&pe()), Some(Danger::Program));
        assert_eq!(content(b"\x7FELF\x02\x01\x01"), Some(Danger::Program));
        assert_eq!(content(&[0xCF, 0xFA, 0xED, 0xFE, 7, 0, 0, 1]), Some(Danger::Program));
        let (dir, path) = scratch("renamed", "holiday.png", &pe());
        assert_eq!(danger_of_path(&path), Some(Danger::Program));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_renamed_script_or_shortcut_is_caught_by_its_content() {
        assert_eq!(content(b"#!/bin/sh\nrm -rf ~\n"), Some(Danger::Script));
        assert_eq!(content(b"\xEF\xBB\xBF#!/usr/bin/env python\n"), Some(Danger::Script));
        assert_eq!(content(b"@ECHO OFF\r\ndel *\r\n"), Some(Danger::Script));
        assert_eq!(content(b"\xFF\xFEW\0i\0n\0d\0o\0w\0s\0 \0R\0e\0g\0i\0s\0t\0r\0y\0 \0E\0d\0i\0t\0o\0r\0"), Some(Danger::Script));
        assert_eq!(content(b"\x4C\x00\x00\x00\x01\x14\x02\x00\x00"), Some(Danger::Shortcut));
        assert_eq!(content(b"[InternetShortcut]\r\nURL=file:///c:/x.exe\r\n"), Some(Danger::Shortcut));
    }

    #[test]
    fn a_docx_with_macros_is_caught_and_one_without_is_not() {
        assert_eq!(content(&zip(&["[Content_Types].xml", "word/document.xml", "word/vbaProject.bin"])), Some(Danger::Macro));
        assert_eq!(content(&zip(&["[Content_Types].xml", "xl/macrosheets/sheet1.xml"])), Some(Danger::Macro));
        assert_eq!(content(&zip(&["[Content_Types].xml", "word/document.xml"])), None);
        let (dir, path) = scratch("macro", "report.docx", &zip(&["word/vbaProject.bin"]));
        assert_eq!(danger_of_path(&path), Some(Danger::Macro));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_old_office_file_with_macros_is_caught_and_one_without_is_not() {
        assert_eq!(content(&ole(&["WordDocument", "Macros/VBA/_VBA_PROJECT"])), Some(Danger::Macro));
        assert_eq!(content(&ole(&["Workbook", "_VBA_PROJECT_CUR/VBA/_VBA_PROJECT"])), Some(Danger::Macro));
        assert_eq!(content(&ole(&["WordDocument"])), None);
    }

    #[test]
    fn a_windows_installer_is_caught_by_its_class_id() {
        let mut file = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        let msi = uuid::Uuid::parse_str("000c1084-0000-0000-c000-000000000046").unwrap();
        file.set_storage_clsid("/", msi).unwrap();
        file.flush().unwrap();
        assert_eq!(content(&file.into_inner().into_inner()), Some(Danger::Installer));
    }

    #[test]
    fn programs_in_a_zip_coat_are_caught() {
        assert_eq!(content(&zip(&["META-INF/MANIFEST.MF", "a/Main.class"])), Some(Danger::Program));
        assert_eq!(content(&zip(&["AndroidManifest.xml", "classes.dex"])), Some(Danger::Program));
        assert_eq!(content(&zip(&["Payload/Thing.app/Thing"])), Some(Danger::Program));
        assert_eq!(content(&zip(&["photos/a.jpg", "notes.txt"])), None, "a plain zip is not judged by what it holds");
    }

    #[test]
    fn disk_images_are_caught_where_their_signatures_sit() {
        let mut iso = vec![0u8; 0x8100];
        iso[0x8001..0x8006].copy_from_slice(b"CD001");
        assert_eq!(content(&iso), Some(Danger::DiskImage));
        let mut dmg = vec![0u8; 2048];
        dmg[2048 - 512..2048 - 508].copy_from_slice(b"koly");
        assert_eq!(content(&dmg), Some(Danger::DiskImage));
    }

    #[test]
    fn a_broken_container_is_unchecked_rather_than_safe() {
        let (dir, path) = scratch("broken", "report.docx", b"PK\x03\x04 and then nothing a zip needs");
        assert_eq!(danger_of_path(&path), Some(Danger::Unchecked));
        std::fs::write(&path, b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1 not really").unwrap();
        assert_eq!(danger_of_path(&path), Some(Danger::Unchecked));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_zip_directory_that_points_outside_the_file_is_unchecked() {
        let mut bad = zip(&["a.txt"]);
        let end = bad.len() - 22;
        bad[end + 12..end + 16].copy_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
        assert!(danger_of_content(&mut Cursor::new(bad)).is_err());
    }

    #[test]
    fn ordinary_files_pass() {
        assert_eq!(content(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), None);
        assert_eq!(content(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n"), None);
        assert_eq!(content(b"just some notes"), None);
        assert_eq!(content(b""), None);
        let (dir, path) = scratch("safe", "photo.png", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR");
        assert_eq!(danger_of_path(&path), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_file_has_no_verdict() {
        assert_eq!(danger_of_path(&std::env::temp_dir().join("dissent-no-such-file.png")), None);
        assert_eq!(danger_of_path(Path::new("gone.exe")), Some(Danger::Program), "the name still counts");
    }

    #[test]
    fn the_flag_reaches_the_ui_in_snake_case() {
        assert_eq!(serde_json::to_string(&Danger::DiskImage).unwrap(), "\"disk_image\"");
        assert_eq!(serde_json::to_string(&Danger::Macro).unwrap(), "\"macro\"");
    }

    #[cfg(windows)]
    #[test]
    fn a_marked_file_carries_zone_3_and_keeps_its_content() {
        let (dir, path) = scratch("zone", "a.txt", b"body");
        mark_received(&path).unwrap();
        let stream = std::fs::read_to_string(format!("{}:Zone.Identifier", path.display())).unwrap();
        assert!(stream.contains("[ZoneTransfer]") && stream.contains("ZoneId=3"), "{stream}");
        assert_eq!(std::fs::read(&path).unwrap(), b"body");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_failed_mark_keeps_the_file() {
        let (dir, path) = scratch("zone-fail", "a.txt", b"body");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms.clone()).unwrap();
        assert!(mark_received(&path).is_err(), "a read-only file refuses the stream");
        mark_or_log(&path);
        assert_eq!(std::fs::read(&path).unwrap(), b"body");
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&path, perms).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_marked_file_carries_the_quarantine_attribute() {
        let (dir, path) = scratch("quarantine", "a.txt", b"body");
        mark_received(&path).unwrap();
        let value = xattr::get(&path, "com.apple.quarantine").unwrap().expect("the attribute");
        assert!(String::from_utf8_lossy(&value).starts_with("0081;"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
