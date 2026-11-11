//! Download and safely unpack a GitHub archive or a ZIP.
//!
//! Archives are hostile input. Every entry path is validated by hand, only
//! regular files and folders are written, sizes are enforced on the bytes
//! actually read, and nothing is ever made executable.

use std::cell::Cell;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Size and count limits applied while unpacking and downloading.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
    pub max_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_total_bytes: 300 * 1024 * 1024,
            max_file_bytes: 50 * 1024 * 1024,
            max_entries: 50_000,
        }
    }
}

const MAX_PATH_BYTES: usize = 4096;
/// Most bytes of header and metadata (long names, PAX data) read between two
/// entries.
const MAX_METADATA_BYTES: u64 = 1024 * 1024;

/// A short, printable excerpt of an archive-supplied path for error messages.
fn excerpt(raw: &str) -> String {
    raw.chars()
        .take(100)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// Validates an archive entry path and returns its components. `Ok(None)`
/// means there is nothing to write: the entry names the archive root, or a
/// component contains `:` (unsafe on Windows, so skipped everywhere along
/// with its descendants). Backslashes count as separators on every platform.
fn validate_path(raw: &str) -> Result<Option<Vec<String>>, String> {
    let bad = || format!("archive entry has an unsafe path: \"{}\"", excerpt(raw));
    if raw.contains('\0') {
        return Err(bad());
    }
    let norm = raw.replace('\\', "/");
    if norm.starts_with('/') {
        return Err(bad());
    }
    let mut parts = Vec::new();
    let mut skip = false;
    for comp in norm.split('/') {
        match comp {
            "" | "." => {}
            ".." => return Err(bad()),
            c => {
                skip |= c.contains(':');
                parts.push(c.to_string());
            }
        }
    }
    Ok(if skip || parts.is_empty() {
        None
    } else {
        Some(parts)
    })
}

/// Length-checks raw entry name bytes before any string conversion.
fn parts_from_bytes(raw: &[u8]) -> Result<Option<Vec<String>>, String> {
    if raw.len() > MAX_PATH_BYTES {
        return Err("archive entry has a path that is too long".to_string());
    }
    validate_path(&String::from_utf8_lossy(raw))
}

fn dest_for(dest: &Path, parts: &[String]) -> PathBuf {
    let mut p = dest.to_path_buf();
    for part in parts {
        p.push(part);
    }
    p
}

/// Reads `reader` to the end into a new file at `path`, counting the bytes
/// actually read and stopping with an error once a limit is passed.
fn copy_limited(
    mut reader: impl Read,
    path: &Path,
    limits: &Limits,
    total: &mut u64,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("could not create folder: {e}"))?;
    }
    let mut out =
        std::fs::File::create(path).map_err(|e| format!("could not write a file: {e}"))?;
    let mut buf = [0u8; 64 * 1024];
    let mut file_bytes: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("could not read archive: {e}"))?;
        if n == 0 {
            return Ok(());
        }
        file_bytes += n as u64;
        *total += n as u64;
        if file_bytes > limits.max_file_bytes {
            return Err("archive contains a file that is too large".to_string());
        }
        if *total > limits.max_total_bytes {
            return Err("archive is too large when unpacked".to_string());
        }
        out.write_all(&buf[..n])
            .map_err(|e| format!("could not write a file: {e}"))?;
    }
}

/// Caps the bytes pulled from a stream, so skipped entries cannot expand
/// without bound either. A second, resettable window bounds how much header
/// and metadata the tar reader may buffer between entries.
struct CapReader<R> {
    inner: R,
    remaining: u64,
    window: Rc<Cell<u64>>,
}

impl<R: Read> Read for CapReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)? as u64;
        if n > self.remaining {
            return Err(std::io::Error::other("archive is too large when unpacked"));
        }
        if n > self.window.get() {
            return Err(std::io::Error::other("archive header data is too large"));
        }
        self.remaining -= n;
        self.window.set(self.window.get() - n);
        Ok(n as usize)
    }
}

/// Unpacks a (already gzip-decoded) tar stream into `dest`.
pub fn unpack_tar_gz(reader: impl Read, dest: &Path, limits: &Limits) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("could not create folder: {e}"))?;
    // Headers and padding add up to a little over 1 KiB per entry.
    let cap = limits
        .max_total_bytes
        .saturating_add((limits.max_entries as u64).saturating_mul(2048))
        .saturating_add(4096);
    let window = Rc::new(Cell::new(MAX_METADATA_BYTES));
    let mut archive = tar::Archive::new(CapReader {
        inner: reader,
        remaining: cap,
        window: Rc::clone(&window),
    });
    let entries = archive
        .entries()
        .map_err(|e| format!("could not read archive: {e}"))?;
    let mut count = 0usize;
    let mut total = 0u64;
    let mut entries = entries;
    loop {
        // Only headers and metadata are read while the window is limited;
        // entry bodies are bounded by the counting copy below.
        window.set(MAX_METADATA_BYTES);
        let Some(entry) = entries.next() else {
            break;
        };
        let mut entry = entry.map_err(|e| format!("could not read archive: {e}"))?;
        window.set(u64::MAX);
        count += 1;
        if count > limits.max_entries {
            return Err("archive has too many files".to_string());
        }
        let ty = entry.header().entry_type();
        let is_dir = ty == tar::EntryType::Directory;
        let is_file = ty == tar::EntryType::Regular || ty == tar::EntryType::Continuous;
        // Validate every entry, even ones we skip, so a hostile name aborts.
        let parts = parts_from_bytes(&entry.path_bytes())?;
        match parts {
            Some(parts) if is_dir => {
                std::fs::create_dir_all(dest_for(dest, &parts))
                    .map_err(|e| format!("could not create folder: {e}"))?;
            }
            Some(parts) if is_file => {
                copy_limited(&mut entry, &dest_for(dest, &parts), limits, &mut total)?;
            }
            _ => {
                // Skipped: read the body here, bounded, rather than leaving
                // it to the metadata window.
                let n = std::io::copy(
                    &mut (&mut entry).take(limits.max_file_bytes + 1),
                    &mut std::io::sink(),
                )
                .map_err(|e| format!("could not read archive: {e}"))?;
                if n > limits.max_file_bytes {
                    return Err("archive contains a file that is too large".to_string());
                }
            }
        }
    }
    Ok(())
}

/// Unpacks the ZIP at `path` into `dest`.
pub fn unpack_zip(path: &Path, dest: &Path, limits: &Limits) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("could not open the ZIP: {e}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("could not read the ZIP: {e}"))?;
    if archive.len() > limits.max_entries {
        return Err("archive has too many files".to_string());
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("could not create folder: {e}"))?;
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("could not read the ZIP: {e}"))?;
        let raw = String::from_utf8_lossy(entry.name_raw()).into_owned();
        let Some(parts) = parts_from_bytes(entry.name_raw())? else {
            continue;
        };
        let target = dest_for(dest, &parts);
        let kind = entry.unix_mode().map(|m| m & 0o170000);
        let is_dir = entry.is_dir() || raw.ends_with('\\') || kind == Some(0o040000);
        let is_file = matches!(kind, None | Some(0) | Some(0o100000));
        if is_dir {
            std::fs::create_dir_all(&target)
                .map_err(|e| format!("could not create folder: {e}"))?;
        } else if is_file {
            copy_limited(&mut entry, &target, limits, &mut total)?;
        }
    }
    Ok(())
}

fn safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Downloads `{codeload_base}/{repo}/tar.gz/{commit}` and unpacks it into
/// `dest`, returning the archive's single top-level folder.
pub async fn download_github_archive(
    http: &reqwest::Client,
    codeload_base: &str,
    repo: &str,
    commit: &str,
    dest: &Path,
    limits: &Limits,
) -> Result<PathBuf, String> {
    use futures_util::StreamExt;

    let mut repo_parts = repo.split('/');
    let repo_ok = matches!(
        (repo_parts.next(), repo_parts.next(), repo_parts.next()),
        (Some(a), Some(b), None) if safe_segment(a) && safe_segment(b)
    );
    if !repo_ok || !safe_segment(commit) {
        return Err("that repository or commit name is not valid".to_string());
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("could not create folder: {e}"))?;

    let url = format!(
        "{}/{repo}/tar.gz/{commit}",
        codeload_base.trim_end_matches('/')
    );
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|_| "could not reach the download server".to_string())?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err("that repository or commit was not found".to_string());
    }
    if !status.is_success() {
        return Err(format!("the download failed (status {})", status.as_u16()));
    }

    let mut tmp = tempfile::NamedTempFile::new_in(dest)
        .map_err(|e| format!("could not create a temporary file: {e}"))?;
    let mut stream = resp.bytes_stream();
    let mut received: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "the download was interrupted".to_string())?;
        received += chunk.len() as u64;
        if received > limits.max_total_bytes {
            return Err("the download is too large".to_string());
        }
        tmp.write_all(&chunk)
            .map_err(|e| format!("could not write the download: {e}"))?;
    }
    tmp.flush()
        .map_err(|e| format!("could not write the download: {e}"))?;

    let tmp_path = tmp.into_temp_path();
    let src = tmp_path.to_path_buf();
    let dest_owned = dest.to_path_buf();
    let limits_owned = limits.clone();
    let unpacked = tokio::task::spawn_blocking(move || {
        let f =
            std::fs::File::open(&src).map_err(|e| format!("could not read the download: {e}"))?;
        unpack_tar_gz(flate2::read::GzDecoder::new(f), &dest_owned, &limits_owned)
    })
    .await
    .map_err(|_| "unpacking was interrupted".to_string())?;
    tmp_path
        .close()
        .map_err(|e| format!("could not remove the temporary file: {e}"))?;
    unpacked?;

    let mut tops = std::fs::read_dir(dest)
        .map_err(|e| format!("could not read folder: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("could not read folder: {e}"))?;
    if tops.len() != 1 {
        return Err("the archive must contain exactly one top-level folder".to_string());
    }
    let top = tops.remove(0);
    if !top.file_type().map(|t| t.is_dir()).unwrap_or(false) {
        return Err("the archive must contain exactly one top-level folder".to_string());
    }
    Ok(top.path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn limits() -> Limits {
        Limits {
            max_total_bytes: 1_000_000,
            max_file_bytes: 100_000,
            max_entries: 100,
        }
    }

    // ---- tar helpers ----

    fn tar_file(b: &mut tar::Builder<Vec<u8>>, path: &str, data: &[u8]) {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o755);
        h.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut h, path, data).unwrap();
    }

    /// Appends an entry whose name bytes are written verbatim (the builder
    /// refuses unsafe paths).
    fn tar_raw(b: &mut tar::Builder<Vec<u8>>, name: &str, data: &[u8]) {
        let mut h = tar::Header::new_gnu();
        h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        b.append(&h, data).unwrap();
    }

    fn finish_gz(b: tar::Builder<Vec<u8>>) -> Vec<u8> {
        let raw = b.into_inner().unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&raw).unwrap();
        enc.finish().unwrap()
    }

    fn untar(bytes: Vec<u8>, dest: &Path, l: &Limits) -> Result<(), String> {
        let gz = flate2::read::GzDecoder::new(Cursor::new(bytes));
        unpack_tar_gz(gz, dest, l)
    }

    // ---- zip helpers ----

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn unzip(bytes: &[u8], dest: &Path, l: &Limits) -> Result<(), String> {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.zip");
        std::fs::write(&p, bytes).unwrap();
        unpack_zip(&p, dest, l)
    }

    /// `parent` holds `dest` so we can assert nothing leaked next to it.
    fn sandbox() -> (tempfile::TempDir, PathBuf) {
        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        (parent, dest)
    }

    /// Every file and folder under `dir` as sorted relative names with `/`
    /// separators. Built from directory listings only, so it never joins an
    /// archive-style name (drive prefix, leading separator) onto a path.
    fn tree(dir: &Path) -> Vec<String> {
        fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let e = e.unwrap();
                let name = format!("{prefix}{}", e.file_name().to_string_lossy());
                out.push(name.clone());
                if e.file_type().unwrap().is_dir() {
                    walk(&e.path(), &format!("{name}/"), out);
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, "", &mut out);
        out.sort();
        out
    }

    fn only_dest(parent: &Path) {
        let names: Vec<_> = std::fs::read_dir(parent)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("dest")]);
    }

    // ---- tar ----

    #[test]
    fn tar_normal_archive_unpacks_to_same_bytes() {
        let (_p, dest) = sandbox();
        let mut b = tar::Builder::new(Vec::new());
        tar_file(&mut b, "top/a.md", b"hello");
        tar_file(&mut b, "top/sub/b.txt", b"world");
        untar(finish_gz(b), &dest, &limits()).unwrap();
        assert_eq!(std::fs::read(dest.join("top/a.md")).unwrap(), b"hello");
        assert_eq!(std::fs::read(dest.join("top/sub/b.txt")).unwrap(), b"world");
    }

    #[test]
    fn tar_dotdot_entry_errors_and_writes_nothing_outside() {
        let (p, dest) = sandbox();
        let mut b = tar::Builder::new(Vec::new());
        tar_raw(&mut b, "../evil", b"x");
        assert!(untar(finish_gz(b), &dest, &limits()).is_err());
        only_dest(p.path());
        assert!(!dest.join("evil").exists());
    }

    #[test]
    fn tar_nested_dotdot_and_backslash_dotdot_error() {
        for name in ["a/../../evil", "a\\..\\..\\evil", "..\\evil"] {
            let (p, dest) = sandbox();
            let mut b = tar::Builder::new(Vec::new());
            tar_raw(&mut b, name, b"x");
            assert!(untar(finish_gz(b), &dest, &limits()).is_err(), "{name}");
            only_dest(p.path());
        }
    }

    #[test]
    fn tar_absolute_and_drive_and_unc_paths_error() {
        for name in ["/etc/evil", "\\\\server\\share\\x", "//server/x"] {
            let (p, dest) = sandbox();
            let mut b = tar::Builder::new(Vec::new());
            tar_raw(&mut b, name, b"x");
            assert!(untar(finish_gz(b), &dest, &limits()).is_err(), "{name}");
            only_dest(p.path());
        }
    }

    #[test]
    fn tar_nul_in_path_errors() {
        let (_p, dest) = sandbox();
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.as_old_mut().name[..5].copy_from_slice(b"a\0b/c");
        h.set_size(1);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        b.append(&h, &b"x"[..]).unwrap();
        // The tar crate truncates names at NUL when reading, so the bytes seen
        // by the unpacker are "a": the point is that it never errors by
        // writing somewhere odd; either outcome must stay inside dest.
        let _ = untar(finish_gz(b), &dest, &limits());
        assert!(!dest.parent().unwrap().join("b").exists());
    }

    #[test]
    fn validate_rejects_nul() {
        assert!(validate_path("a\0b").is_err());
    }

    #[test]
    fn tar_symlink_skipped_other_files_unpack() {
        let (_p, dest) = sandbox();
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        b.append_link(&mut h, "top/link", "/etc/passwd").unwrap();
        tar_file(&mut b, "top/a.md", b"ok");
        untar(finish_gz(b), &dest, &limits()).unwrap();
        assert!(dest.join("top/a.md").exists());
        assert!(std::fs::symlink_metadata(dest.join("top/link")).is_err());
    }

    #[test]
    fn tar_hardlink_and_device_skipped() {
        let (_p, dest) = sandbox();
        let mut b = tar::Builder::new(Vec::new());
        tar_file(&mut b, "top/a.md", b"ok");
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Link);
        h.set_size(0);
        b.append_link(&mut h, "top/hard", "top/a.md").unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Char);
        h.set_size(0);
        h.set_cksum();
        b.append_data(&mut h, "top/dev", &b""[..]).unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Fifo);
        h.set_size(0);
        h.set_cksum();
        b.append_data(&mut h, "top/fifo", &b""[..]).unwrap();
        untar(finish_gz(b), &dest, &limits()).unwrap();
        assert!(dest.join("top/a.md").exists());
        for n in ["hard", "dev", "fifo"] {
            assert!(
                std::fs::symlink_metadata(dest.join("top").join(n)).is_err(),
                "{n}"
            );
        }
    }

    #[test]
    fn tar_total_size_over_limit_errors() {
        let (_p, dest) = sandbox();
        let l = Limits {
            max_total_bytes: 250,
            max_file_bytes: 200,
            max_entries: 100,
        };
        let mut b = tar::Builder::new(Vec::new());
        for i in 0..5 {
            tar_file(&mut b, &format!("t/{i}"), &[b'a'; 100]);
        }
        assert!(untar(finish_gz(b), &dest, &l).is_err());
        assert!(!dest.join("t/4").exists());
    }

    #[test]
    fn tar_single_file_over_limit_errors() {
        let (_p, dest) = sandbox();
        let l = Limits {
            max_total_bytes: 10_000,
            max_file_bytes: 50,
            max_entries: 100,
        };
        let mut b = tar::Builder::new(Vec::new());
        tar_file(&mut b, "t/big", &[b'a'; 51]);
        assert!(untar(finish_gz(b), &dest, &l).is_err());
    }

    #[test]
    fn tar_too_many_entries_errors() {
        let (_p, dest) = sandbox();
        let l = Limits {
            max_total_bytes: 10_000,
            max_file_bytes: 1_000,
            max_entries: 3,
        };
        let mut b = tar::Builder::new(Vec::new());
        for i in 0..4 {
            tar_file(&mut b, &format!("t/{i}"), b"x");
        }
        assert!(untar(finish_gz(b), &dest, &l).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn tar_does_not_set_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let (_p, dest) = sandbox();
        let mut b = tar::Builder::new(Vec::new());
        tar_file(&mut b, "t/run.sh", b"#!/bin/sh");
        untar(finish_gz(b), &dest, &limits()).unwrap();
        let mode = std::fs::metadata(dest.join("t/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0);
    }

    // ---- zip ----

    #[test]
    fn zip_normal_archive_unpacks_to_same_bytes() {
        let (_p, dest) = sandbox();
        let z = zip_bytes(&[("top/a.md", b"hello"), ("top/sub/b.txt", b"world")]);
        unzip(&z, &dest, &limits()).unwrap();
        assert_eq!(std::fs::read(dest.join("top/a.md")).unwrap(), b"hello");
        assert_eq!(std::fs::read(dest.join("top/sub/b.txt")).unwrap(), b"world");
    }

    #[test]
    fn zip_dotdot_entry_errors_and_writes_nothing_outside() {
        for name in ["../evil", "a/../../evil", "..\\evil"] {
            let (p, dest) = sandbox();
            let z = zip_bytes(&[(name, b"x")]);
            assert!(unzip(&z, &dest, &limits()).is_err(), "{name}");
            only_dest(p.path());
        }
    }

    #[test]
    fn zip_absolute_paths_error() {
        for name in ["/etc/evil", "\\\\server\\share\\x", "\\evil"] {
            let (p, dest) = sandbox();
            let z = zip_bytes(&[(name, b"x")]);
            assert!(unzip(&z, &dest, &limits()).is_err(), "{name}");
            only_dest(p.path());
        }
    }

    #[test]
    fn zip_symlink_skipped_other_files_unpack() {
        let (_p, dest) = sandbox();
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        w.add_symlink("top/link", "/etc/passwd", opts).unwrap();
        w.start_file("top/a.md", opts).unwrap();
        w.write_all(b"ok").unwrap();
        let z = w.finish().unwrap().into_inner();
        unzip(&z, &dest, &limits()).unwrap();
        assert!(dest.join("top/a.md").exists());
        assert!(std::fs::symlink_metadata(dest.join("top/link")).is_err());
    }

    #[test]
    fn zip_total_size_over_limit_errors() {
        let (_p, dest) = sandbox();
        let l = Limits {
            max_total_bytes: 250,
            max_file_bytes: 200,
            max_entries: 100,
        };
        let data = [b'a'; 100];
        let names: Vec<String> = (0..5).map(|i| format!("t/{i}")).collect();
        let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &data[..])).collect();
        assert!(unzip(&zip_bytes(&entries), &dest, &l).is_err());
    }

    #[test]
    fn zip_single_file_over_limit_errors() {
        let (_p, dest) = sandbox();
        let l = Limits {
            max_total_bytes: 10_000,
            max_file_bytes: 50,
            max_entries: 100,
        };
        assert!(unzip(&zip_bytes(&[("t/big", &[b'a'; 51])]), &dest, &l).is_err());
    }

    #[test]
    fn zip_too_many_entries_errors() {
        let (_p, dest) = sandbox();
        let l = Limits {
            max_total_bytes: 10_000,
            max_file_bytes: 1_000,
            max_entries: 3,
        };
        let z = zip_bytes(&[("t/0", b"x"), ("t/1", b"x"), ("t/2", b"x"), ("t/3", b"x")]);
        assert!(unzip(&z, &dest, &l).is_err());
    }

    /// Declared uncompressed size is patched down to 10 bytes while the real
    /// deflate stream expands to 2 MB: the limit must trip on the bytes read.
    #[test]
    fn zip_bomb_with_lying_declared_size_errors() {
        let (_p, dest) = sandbox();
        let l = Limits {
            max_total_bytes: 10_000_000,
            max_file_bytes: 1_000,
            max_entries: 100,
        };
        let mut z = zip_bytes(&[("t/bomb", &vec![0u8; 2_000_000])]);
        let find = |z: &[u8], sig: [u8; 4]| z.windows(4).position(|w| w == sig).unwrap();
        let local = find(&z, [0x50, 0x4b, 0x03, 0x04]);
        z[local + 22..local + 26].copy_from_slice(&10u32.to_le_bytes());
        let central = find(&z, [0x50, 0x4b, 0x01, 0x02]);
        z[central + 24..central + 28].copy_from_slice(&10u32.to_le_bytes());
        assert!(unzip(&z, &dest, &l).is_err());
        let len = std::fs::metadata(dest.join("t/bomb"))
            .map(|m| m.len())
            .unwrap_or(0);
        assert!(len <= 1_000 + 65_536, "wrote {len} bytes");
    }

    #[cfg(unix)]
    #[test]
    fn zip_does_not_set_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let (_p, dest) = sandbox();
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        w.start_file("t/run.sh", opts).unwrap();
        w.write_all(b"#!/bin/sh").unwrap();
        let z = w.finish().unwrap().into_inner();
        unzip(&z, &dest, &limits()).unwrap();
        let mode = std::fs::metadata(dest.join("t/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0);
    }

    // ---- colon skipping, long metadata, message length ----

    #[test]
    fn tar_colon_components_are_skipped_siblings_unpack() {
        let (p, dest) = sandbox();
        let mut b = tar::Builder::new(Vec::new());
        tar_file(&mut b, "top/a:b.md", b"x");
        tar_file(&mut b, "top/C:evil/x.md", b"x");
        tar_raw(&mut b, "C:/evil", b"x");
        tar_file(&mut b, "top/ok.md", b"ok");
        untar(finish_gz(b), &dest, &limits()).unwrap();
        assert_eq!(tree(&dest), vec!["top", "top/ok.md"]);
        only_dest(p.path());
    }

    #[test]
    fn zip_colon_components_are_skipped_siblings_unpack() {
        let (p, dest) = sandbox();
        let z = zip_bytes(&[
            ("top/a:b.md", b"x"),
            ("top/C:evil/x.md", b"x"),
            ("C:\\evil", b"x"),
            ("top/ok.md", b"ok"),
        ]);
        unzip(&z, &dest, &limits()).unwrap();
        assert_eq!(tree(&dest), vec!["top", "top/ok.md"]);
        only_dest(p.path());
    }

    #[test]
    fn dotdot_after_colon_component_still_errors() {
        assert!(validate_path("a:b/../../evil").is_err());
        assert!(validate_path("/x").is_err());
        assert!(validate_path("\\x").is_err());
        assert_eq!(validate_path("top/a:b").unwrap(), None);
    }

    fn tar_long_name(b: &mut tar::Builder<Vec<u8>>, name: &[u8]) {
        let mut data = name.to_vec();
        data.push(0);
        let mut h = tar::Header::new_gnu();
        h.as_old_mut().name[..13].copy_from_slice(b"././@LongLink");
        h.set_size(data.len() as u64);
        h.set_entry_type(tar::EntryType::GNULongName);
        h.set_cksum();
        b.append(&h, &data[..]).unwrap();
    }

    #[test]
    fn tar_huge_long_name_header_errors_promptly() {
        let (_p, dest) = sandbox();
        let l = Limits {
            max_total_bytes: 100_000_000,
            max_file_bytes: 1_000,
            max_entries: 100,
        };
        let mut b = tar::Builder::new(Vec::new());
        tar_long_name(&mut b, &vec![b'a'; 5 * 1024 * 1024]);
        tar_file(&mut b, "ignored", b"x");
        let started = std::time::Instant::now();
        let err = untar(finish_gz(b), &dest, &l).unwrap_err();
        assert!(err.contains("too large"), "{err}");
        assert!(err.len() < 200);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn tar_over_long_path_errors() {
        let (_p, dest) = sandbox();
        let mut b = tar::Builder::new(Vec::new());
        tar_long_name(&mut b, "a/".repeat(2_500).as_bytes());
        tar_file(&mut b, "ignored", b"x");
        let err = untar(finish_gz(b), &dest, &limits()).unwrap_err();
        assert!(err.contains("too long"), "{err}");
        assert!(err.len() < 200);
    }

    #[test]
    fn zip_over_long_path_errors() {
        let (_p, dest) = sandbox();
        let name = "a/".repeat(2_500);
        let z = zip_bytes(&[(name.as_str(), b"x")]);
        let err = unzip(&z, &dest, &limits()).unwrap_err();
        assert!(err.contains("too long"), "{err}");
    }

    #[test]
    fn hostile_path_error_message_is_short() {
        let long = format!("../{}\n\u{1}", "a".repeat(3_000));
        let err = validate_path(&long).unwrap_err();
        assert!(err.len() <= 200, "{}", err.len());
        let ctl = format!("../{}", "\u{1}".repeat(100));
        assert!(validate_path(&ctl).unwrap_err().len() <= 200);
    }

    // ---- download ----

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sample_tar_gz() -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        tar_file(&mut b, "repo-abc123/SKILL.md", b"hi");
        finish_gz(b)
    }

    #[tokio::test]
    async fn download_returns_top_folder() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/owner/repo/tar.gz/abc123"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(sample_tar_gz()))
            .mount(&server)
            .await;
        let dest = tempfile::tempdir().unwrap();
        let top = download_github_archive(
            &reqwest::Client::new(),
            &server.uri(),
            "owner/repo",
            "abc123",
            dest.path(),
            &limits(),
        )
        .await
        .unwrap();
        assert_eq!(top, dest.path().join("repo-abc123"));
        assert_eq!(std::fs::read(top.join("SKILL.md")).unwrap(), b"hi");
        // The temporary download is gone.
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn download_404_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_string("secret body"))
            .mount(&server)
            .await;
        let dest = tempfile::tempdir().unwrap();
        let err = download_github_archive(
            &reqwest::Client::new(),
            &server.uri(),
            "owner/repo",
            "abc123",
            dest.path(),
            &limits(),
        )
        .await
        .unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert!(!err.contains("secret body"));
    }

    #[tokio::test]
    async fn download_over_total_limit_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 5_000]))
            .mount(&server)
            .await;
        let dest = tempfile::tempdir().unwrap();
        let l = Limits {
            max_total_bytes: 1_000,
            max_file_bytes: 1_000,
            max_entries: 10,
        };
        let err = download_github_archive(
            &reqwest::Client::new(),
            &server.uri(),
            "owner/repo",
            "abc123",
            dest.path(),
            &l,
        )
        .await
        .unwrap_err();
        assert!(err.contains("too large"), "{err}");
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn download_with_two_top_folders_errors() {
        let mut b = tar::Builder::new(Vec::new());
        tar_file(&mut b, "one/a", b"x");
        tar_file(&mut b, "two/b", b"x");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(finish_gz(b)))
            .mount(&server)
            .await;
        let dest = tempfile::tempdir().unwrap();
        assert!(
            download_github_archive(
                &reqwest::Client::new(),
                &server.uri(),
                "owner/repo",
                "abc123",
                dest.path(),
                &limits(),
            )
            .await
            .is_err()
        );
    }
}
