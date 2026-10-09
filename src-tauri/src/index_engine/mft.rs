//! Windows NTFS index source based on FSCTL_ENUM_USN_DATA.
//!
//! NTFS enumeration avoids opening every directory entry. Non-NTFS volumes and
//! volumes where access is denied transparently use the existing walkdir scan.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::fs_core::{path_id, to_entry};
use crate::index_engine::{IndexSource, WalkDirSource};
use crate::types::FileEntry;
use crate::persistence::IndexJournal;
use dashmap::DashMap;
use parking_lot::Mutex;
use std::time::Instant;

const FSCTL_ENUM_USN_DATA: u32 = 0x0009_00B3;
const FSCTL_READ_USN_JOURNAL: u32 = 0x0009_00BB;
const FSCTL_QUERY_USN_JOURNAL: u32 = 0x0009_00F4;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const GENERIC_READ: u32 = 0x8000_0000;
const INVALID_HANDLE_VALUE: isize = -1;
const BUFFER_SIZE: usize = 1024 * 1024;
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;
const REASON_FILE_DELETE: u32 = 0x0000_0200;
const REASON_RENAME_OLD_NAME: u32 = 0x0000_1000;

#[repr(C)]
struct ReadUsnJournalData {
    start_usn: i64,
    reason_mask: u32,
    return_only_on_close: u32,
    timeout: u64,
    bytes_to_wait_for: u64,
    journal_id: u64,
}

#[repr(C)]
struct FileIdDescriptor {
    size: u32,
    kind: u32,
    id: [u64; 2],
}

#[repr(C)]
struct MftEnumData {
    start_file_reference_number: u64,
    low_usn: i64,
    high_usn: i64,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        creation: u32,
        flags: u32,
        template: isize,
    ) -> isize;
    fn DeviceIoControl(
        handle: isize,
        control: u32,
        input: *const c_void,
        input_len: u32,
        output: *mut c_void,
        output_len: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn CloseHandle(handle: isize) -> i32;
    fn OpenFileById(
        volume: isize,
        id: *const FileIdDescriptor,
        access: u32,
        share: u32,
        security: *const c_void,
        flags: u32,
    ) -> isize;
    fn GetFinalPathNameByHandleW(handle: isize, path: *mut u16, len: u32, flags: u32) -> u32;
    fn GetVolumeInformationW(
        root: *const u16,
        name: *mut u16,
        name_len: u32,
        serial: *mut u32,
        max_component: *mut u32,
        flags: *mut u32,
        filesystem: *mut u16,
        filesystem_len: u32,
    ) -> i32;
}

/// Follow the NTFS change journal after the initial scan and keep the in-memory index current.
pub(super) fn watch_roots(
    roots: Vec<String>,
    entries: Arc<DashMap<String, FileEntry>>,
    generation: u64,
    current_generation: Arc<AtomicU64>,
    checkpoints: Vec<IndexJournal>,
) {
    let mut watchers = Vec::new();
    let checkpoints = Arc::new(Mutex::new(checkpoints));
    let last_saved = Arc::new(Mutex::new(Instant::now()));
    for root in &roots {
        if current_generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let Some(volume) = volume_root(root) else {
            continue;
        };
        let entries = Arc::clone(&entries);
        let current_generation = Arc::clone(&current_generation);
        let checkpoint = checkpoints
            .lock()
            .iter()
            .find(|checkpoint| checkpoint.volume.eq_ignore_ascii_case(&volume))
            .cloned();
        let start_usn = checkpoint.as_ref().map(|checkpoint| checkpoint.next_usn);
        let journal_id = checkpoint.as_ref().map(|checkpoint| checkpoint.journal_id);
        let checkpoints = Arc::clone(&checkpoints);
        let last_saved = Arc::clone(&last_saved);
        let all_roots = roots.clone();
        let root = root.clone();
        watchers.push(std::thread::spawn(move || {
            watch_volume(
                &root,
                &volume,
                &entries,
                generation,
                &current_generation,
                start_usn,
                journal_id,
                &checkpoints,
                &last_saved,
                &all_roots,
            );
        }));
    }
    for watcher in watchers {
        let _ = watcher.join();
    }
}

fn watch_volume(
    root: &str,
    volume: &str,
    entries: &DashMap<String, FileEntry>,
    generation: u64,
    current_generation: &AtomicU64,
    start_usn: Option<i64>,
    expected_journal_id: Option<u64>,
    checkpoints: &Mutex<Vec<IndexJournal>>,
    last_saved: &Mutex<Instant>,
    roots: &[String],
) {
    let device: Vec<u16> = format!("\\\\.\\{}", &volume[0..2])
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            device.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return;
    }

    let mut journal = [0u8; 64];
    let mut returned = 0u32;
    let queried = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_QUERY_USN_JOURNAL,
            std::ptr::null(),
            0,
            journal.as_mut_ptr() as *mut c_void,
            journal.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if queried == 0 || returned < 24 {
        unsafe {
            CloseHandle(handle);
        }
        return;
    }
    let journal_id = read_u64(&journal, 0);
    if expected_journal_id.is_some_and(|expected| expected != journal_id) {
        unsafe { CloseHandle(handle) };
        return;
    }
    // A persisted checkpoint starts at the exact point captured by the index service.
    // Without one, replay the retained journal to cover changes made during a full scan.
    let mut cursor = start_usn.unwrap_or_else(|| read_u64(&journal, 8) as i64);
    let mut buffer = vec![0u8; BUFFER_SIZE];

    while current_generation.load(Ordering::SeqCst) == generation {
        let input = ReadUsnJournalData {
            start_usn: cursor,
            reason_mask: u32::MAX,
            return_only_on_close: 0,
            timeout: 0,
            bytes_to_wait_for: 0,
            journal_id,
        };
        returned = 0;
        let success = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_READ_USN_JOURNAL,
                &input as *const _ as *const c_void,
                std::mem::size_of::<ReadUsnJournalData>() as u32,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if success != 0 && returned >= 8 {
            cursor = read_u64(&buffer, 0) as i64;
            let mut offset = 8usize;
            while offset + 60 <= returned as usize {
                let length = read_u32(&buffer, offset) as usize;
                if length < 60 || offset + length > returned as usize {
                    break;
                }
                if read_u16(&buffer, offset + 4) == 2 {
                    apply_usn_record(&buffer[offset..offset + length], root, volume, entries);
                }
                offset += length;
            }
            let checkpoint_updated = {
                let mut checkpoints = checkpoints.lock();
                if let Some(checkpoint) = checkpoints
                    .iter_mut()
                    .find(|checkpoint| checkpoint.volume.eq_ignore_ascii_case(volume))
                {
                    checkpoint.next_usn = cursor;
                    true
                } else {
                    false
                }
            };
            if checkpoint_updated {
                let mut last_saved = last_saved.lock();
                if last_saved.elapsed() >= Duration::from_secs(600) {
                    let snapshot = checkpoints.lock().clone();
                    super::persist_index(roots, entries, &snapshot);
                    *last_saved = Instant::now();
                }
            }
        } else {
            // Journal may have wrapped or been recreated. Re-query and resume at its oldest
            // retained record; an index rebuild can be triggered by the user if history is lost.
            let mut latest = [0u8; 64];
            let mut len = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    handle,
                    FSCTL_QUERY_USN_JOURNAL,
                    std::ptr::null(),
                    0,
                    latest.as_mut_ptr() as *mut c_void,
                    latest.len() as u32,
                    &mut len,
                    std::ptr::null_mut(),
                )
            };
            if ok != 0 && len >= 24 {
                let next_id = read_u64(&latest, 0);
                cursor = read_u64(&latest, 8) as i64;
                if next_id != journal_id {
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    unsafe {
        CloseHandle(handle);
    }
}

/// Capture NTFS journal cursors before a full MFT enumeration begins.
pub(super) fn capture_journals(roots: &[String]) -> Option<Vec<IndexJournal>> {
    let mut checkpoints = Vec::new();
    for root in roots {
        let volume = volume_root(root)?;
        if checkpoints
            .iter()
            .any(|checkpoint: &IndexJournal| checkpoint.volume.eq_ignore_ascii_case(&volume))
        {
            continue;
        }
        if let Some((journal_id, _, next_usn)) = query_journal(&volume) {
            checkpoints.push(IndexJournal {
                volume,
                journal_id,
                next_usn,
            });
        }
    }
    Some(checkpoints)
}

/// Replay changes since the saved cursor. `None` means the journal can no longer
/// provide a complete history, so the caller must rebuild the index from MFT.
pub(super) fn resume_journals(
    roots: &[String],
    checkpoints: &[IndexJournal],
    entries: &DashMap<String, FileEntry>,
    scan_untracked_roots: bool,
) -> Option<Vec<IndexJournal>> {
    let mut updated = Vec::new();
    for root in roots {
        let volume = volume_root(root)?;
        if updated
            .iter()
            .any(|checkpoint: &IndexJournal| checkpoint.volume.eq_ignore_ascii_case(&volume))
        {
            continue;
        }
        let Some(saved) = checkpoints
            .iter()
            .find(|checkpoint| checkpoint.volume.eq_ignore_ascii_case(&volume))
        else {
            if scan_untracked_roots {
                entries.retain(|path, _| !is_below_root(path, root));
                WalkDirSource::default().scan(std::slice::from_ref(root), &mut |batch| {
                    for entry in batch {
                        entries.insert(entry.path.clone(), entry);
                    }
                });
            }
            continue;
        };
        let (journal_id, first_usn, next_usn) = query_journal(&volume)?;
        if journal_id != saved.journal_id || saved.next_usn < first_usn || saved.next_usn > next_usn {
            return None;
        }
        let cursor = replay_journal(root, &volume, saved.next_usn, next_usn, journal_id, entries)?;
        updated.push(IndexJournal {
            volume,
            journal_id,
            next_usn: cursor,
        });
    }
    Some(updated)
}

fn query_journal(volume: &str) -> Option<(u64, i64, i64)> {
    let device: Vec<u16> = format!("\\\\.\\{}", &volume[0..2])
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            device.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut journal = [0u8; 64];
    let mut returned = 0u32;
    let success = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_QUERY_USN_JOURNAL,
            std::ptr::null(),
            0,
            journal.as_mut_ptr() as *mut c_void,
            journal.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    } != 0;
    unsafe { CloseHandle(handle) };
    if !success || returned < 24 {
        return None;
    }
    Some((
        read_u64(&journal, 0),
        read_u64(&journal, 8) as i64,
        read_u64(&journal, 16) as i64,
    ))
}

fn replay_journal(
    root: &str,
    volume: &str,
    mut cursor: i64,
    target: i64,
    journal_id: u64,
    entries: &DashMap<String, FileEntry>,
) -> Option<i64> {
    let device: Vec<u16> = format!("\\\\.\\{}", &volume[0..2])
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            device.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut buffer = vec![0u8; BUFFER_SIZE];
    while cursor < target {
        let input = ReadUsnJournalData {
            start_usn: cursor,
            reason_mask: u32::MAX,
            return_only_on_close: 0,
            timeout: 0,
            bytes_to_wait_for: 0,
            journal_id,
        };
        let mut returned = 0u32;
        let success = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_READ_USN_JOURNAL,
                &input as *const _ as *const c_void,
                std::mem::size_of::<ReadUsnJournalData>() as u32,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        } != 0;
        if !success || returned < 8 {
            unsafe { CloseHandle(handle) };
            return None;
        }
        let next = read_u64(&buffer, 0) as i64;
        if next <= cursor {
            break;
        }
        let mut offset = 8usize;
        while offset + 60 <= returned as usize {
            let length = read_u32(&buffer, offset) as usize;
            if length < 60 || offset + length > returned as usize {
                break;
            }
            if read_u16(&buffer, offset + 4) == 2 {
                apply_usn_record(&buffer[offset..offset + length], root, volume, entries);
            }
            offset += length;
        }
        cursor = next;
    }
    unsafe { CloseHandle(handle) };
    (cursor >= target).then_some(cursor)
}

fn apply_usn_record(record: &[u8], root: &str, volume: &str, entries: &DashMap<String, FileEntry>) {
    let name_len = read_u16(record, 56) as usize;
    let name_offset = read_u16(record, 58) as usize;
    if name_len == 0 || name_offset + name_len > record.len() {
        return;
    }
    let name = String::from_utf16_lossy(bytemuck_u16(&record[name_offset..name_offset + name_len]));
    let parent = read_u64(record, 16) & 0x0000_ffff_ffff_ffff;
    let parent_path = match path_for_file_id(volume, parent) {
        Some(p) => p,
        None => return,
    };
    let path = format!("{}\\{}", parent_path.trim_end_matches(['\\', '/']), name);
    if !is_below_root(&path, root) {
        return;
    }
    let reason = read_u32(record, 40);
    if reason & (REASON_FILE_DELETE | REASON_RENAME_OLD_NAME) != 0 {
        let prefix = format!("{}\\", path.trim_end_matches(['\\', '/'])).to_lowercase();
        let path_lower = path.to_lowercase();
        entries.retain(|key, _| {
            let key_lower = key.to_lowercase();
            key_lower != path_lower && !key_lower.starts_with(&prefix)
        });
        return;
    }
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        let entry = to_entry(Path::new(&path), &metadata);
        entries.insert(path, entry);
    }
}

fn path_for_file_id(volume: &str, file_id: u64) -> Option<String> {
    let device: Vec<u16> = format!("\\\\.\\{}", &volume[0..2])
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let volume_handle = unsafe {
        CreateFileW(
            device.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if volume_handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let descriptor = FileIdDescriptor {
        size: std::mem::size_of::<FileIdDescriptor>() as u32,
        kind: 0,
        id: [file_id, 0],
    };
    let file = unsafe {
        OpenFileById(
            volume_handle,
            &descriptor,
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            0,
        )
    };
    unsafe {
        CloseHandle(volume_handle);
    }
    if file == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut path = vec![0u16; 32768];
    let len = unsafe { GetFinalPathNameByHandleW(file, path.as_mut_ptr(), path.len() as u32, 0) };
    unsafe {
        CloseHandle(file);
    }
    if len == 0 || len as usize >= path.len() {
        return None;
    }
    let mut result = String::from_utf16_lossy(&path[..len as usize]);
    if let Some(rest) = result.strip_prefix("\\\\?\\") {
        result = rest.to_string();
    }
    Some(result)
}

#[derive(Default)]
pub(super) struct MftOrWalkSource;

impl IndexSource for MftOrWalkSource {
    fn scan(&self, roots: &[String], on_batch: &mut dyn FnMut(Vec<FileEntry>)) {
        for root in roots {
            if !scan_ntfs_root(root, on_batch) {
                WalkDirSource::default().scan(std::slice::from_ref(root), on_batch);
            }
        }
    }
}

#[derive(Clone)]
struct Record {
    reference: u64,
    parent: u64,
    name: String,
    size: u64,
    modified: i64,
    attributes: u32,
}

fn scan_ntfs_root(root: &str, on_batch: &mut dyn FnMut(Vec<FileEntry>)) -> bool {
    let Some(volume) = volume_root(root) else {
        return false;
    };
    let wide: Vec<u16> = volume.encode_utf16().chain(Some(0)).collect();
    let mut fs_name = [0u16; 32];
    let ntfs = unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_name.as_mut_ptr(),
            fs_name.len() as u32,
        ) != 0
            && String::from_utf16_lossy(&fs_name)
                .to_uppercase()
                .starts_with("NTFS")
    };
    if !ntfs {
        return false;
    }

    let device: Vec<u16> = format!("\\\\.\\{}", &volume[0..2])
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            device.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return false;
    }

    let mut records = HashMap::<u64, Record>::new();
    let mut start = 0u64;
    let mut buffer = vec![0u8; BUFFER_SIZE];
    let mut ok = true;
    loop {
        let input = MftEnumData {
            start_file_reference_number: start,
            low_usn: 0,
            high_usn: i64::MAX,
        };
        let mut returned = 0u32;
        let success = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_ENUM_USN_DATA,
                &input as *const _ as *const c_void,
                std::mem::size_of::<MftEnumData>() as u32,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            ) != 0
        };
        if !success {
            if unsafe { GetLastError() } != 38 {
                // ERROR_HANDLE_EOF
                ok = false;
            }
            break;
        }
        if returned < 8 {
            break;
        }
        start = read_u64(&buffer, 0);
        let mut offset = 8usize;
        while offset + 60 <= returned as usize {
            let length = read_u32(&buffer, offset) as usize;
            if length < 60 || offset + length > returned as usize {
                break;
            }
            let major = read_u16(&buffer, offset + 4);
            if major == 2 {
                let name_len = read_u16(&buffer, offset + 56) as usize;
                let name_offset = read_u16(&buffer, offset + 58) as usize;
                if name_len > 0 && name_offset + name_len <= length {
                    let name = String::from_utf16_lossy(bytemuck_u16(
                        &buffer[offset + name_offset..offset + name_offset + name_len],
                    ));
                    let reference = read_u64(&buffer, offset + 8) & 0x0000_ffff_ffff_ffff;
                    let parent = read_u64(&buffer, offset + 16) & 0x0000_ffff_ffff_ffff;
                    let filetime = read_u64(&buffer, offset + 32);
                    records.insert(
                        reference,
                        Record {
                            reference,
                            parent,
                            name,
                            size: 0,
                            modified: if filetime >= FILETIME_UNIX_EPOCH {
                                ((filetime - FILETIME_UNIX_EPOCH) / 10_000) as i64
                            } else {
                                0
                            },
                            attributes: read_u32(&buffer, offset + 52),
                        },
                    );
                }
            }
            offset += length;
        }
        if start == u64::MAX {
            break;
        }
    }
    unsafe {
        CloseHandle(handle);
    }
    if !ok || records.is_empty() {
        return false;
    }

    let mut paths = HashMap::new();
    let mut batch = Vec::with_capacity(2000);
    for record in records.values() {
        let Some(path) = resolve_path(record.reference, &records, &mut paths, &volume, 0) else {
            continue;
        };
        if !is_below_root(&path, root) {
            continue;
        }
        let is_dir = record.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        let ext = if is_dir {
            None
        } else {
            Path::new(&record.name)
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
        };
        batch.push(FileEntry {
            id: path_id(&path),
            name: record.name.clone(),
            path,
            is_dir,
            size: record.size,
            modified: record.modified,
            ext,
            hidden: record.attributes & FILE_ATTRIBUTE_HIDDEN != 0,
            system: record.attributes & FILE_ATTRIBUTE_SYSTEM != 0,
        });
        if batch.len() == 2000 {
            on_batch(std::mem::take(&mut batch));
            batch = Vec::with_capacity(2000);
        }
    }
    if !batch.is_empty() {
        on_batch(batch);
    }
    true
}

fn resolve_path(
    id: u64,
    records: &HashMap<u64, Record>,
    cache: &mut HashMap<u64, String>,
    volume: &str,
    depth: usize,
) -> Option<String> {
    if let Some(path) = cache.get(&id) {
        return Some(path.clone());
    }
    if depth > 256 {
        return None;
    }
    let record = records.get(&id)?;
    let path = if record.parent == 5 || record.parent == id {
        format!("{}{}", volume, record.name)
    } else {
        format!(
            "{}\\{}",
            resolve_path(record.parent, records, cache, volume, depth + 1)?,
            record.name
        )
    };
    cache.insert(id, path.clone());
    Some(path)
}

fn volume_root(root: &str) -> Option<String> {
    let bytes = root.as_bytes();
    if bytes.len() < 2 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    Some(format!("{}:\\", bytes[0] as char))
}

fn is_below_root(path: &str, root: &str) -> bool {
    let normalized_path = path.trim_end_matches(['\\', '/']).to_lowercase();
    let normalized_root = root.trim_end_matches(['\\', '/']).to_lowercase();
    normalized_path != normalized_root
        && normalized_path.starts_with(&normalized_root)
        && normalized_path
            .as_bytes()
            .get(normalized_root.len())
            .is_some_and(|c| *c == b'\\' || *c == b'/')
}

fn read_u16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn read_u32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}
fn read_u64(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}
fn bytemuck_u16(bytes: &[u8]) -> &[u16] {
    // USN_RECORD_V2 file names are UTF-16 and start at an even byte offset.
    unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u16>(), bytes.len() / 2) }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> u32;
}
