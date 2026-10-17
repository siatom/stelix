//! Windows NTFS index source based on FSCTL_ENUM_USN_DATA.
//!
//! NTFS enumeration avoids opening every directory entry. Non-NTFS volumes and
//! volumes where access is denied transparently use the existing walkdir scan.

use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::fs_core::{path_id, to_entry};
use crate::index_engine::{IndexSource, WalkDirSource};
use crate::persistence::{IndexChange, IndexJournal};
use crate::types::FileEntry;
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
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const GENERIC_READ: u32 = 0x8000_0000;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
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

#[repr(C)]
#[derive(Default)]
struct ByHandleFileInformation {
    attributes: u32,
    creation_time: [u32; 2],
    last_access_time: [u32; 2],
    last_write_time: [u32; 2],
    volume_serial_number: u32,
    file_size_high: u32,
    file_size_low: u32,
    number_of_links: u32,
    file_index_high: u32,
    file_index_low: u32,
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
    fn GetFileInformationByHandle(handle: isize, info: *mut ByHandleFileInformation) -> i32;
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
    catalog_dirty: Arc<AtomicBool>,
    checkpoints: Vec<IndexJournal>,
) {
    let mut watchers = Vec::new();
    let checkpoints = Arc::new(Mutex::new(checkpoints));
    let persisted_checkpoints = Arc::new(Mutex::new(checkpoints.lock().clone()));
    let mut roots_by_volume = HashMap::<String, Vec<String>>::new();
    for root in &roots {
        if let Some(volume) = volume_root(root) {
            roots_by_volume
                .entry(volume)
                .or_default()
                .push(root.clone());
        }
    }
    for (volume, volume_roots) in roots_by_volume {
        if current_generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let entries = Arc::clone(&entries);
        let current_generation = Arc::clone(&current_generation);
        let catalog_dirty = Arc::clone(&catalog_dirty);
        let checkpoint = checkpoints
            .lock()
            .iter()
            .find(|checkpoint| checkpoint.volume.eq_ignore_ascii_case(&volume))
            .cloned();
        let start_usn = checkpoint.as_ref().map(|checkpoint| checkpoint.next_usn);
        let journal_id = checkpoint.as_ref().map(|checkpoint| checkpoint.journal_id);
        let checkpoints = Arc::clone(&checkpoints);
        let persisted_checkpoints = Arc::clone(&persisted_checkpoints);
        let all_roots = roots.clone();
        watchers.push(std::thread::spawn(move || {
            watch_volume(
                &volume_roots,
                &volume,
                &entries,
                generation,
                &current_generation,
                &catalog_dirty,
                start_usn,
                journal_id,
                &checkpoints,
                &persisted_checkpoints,
                &all_roots,
            );
        }));
    }
    for watcher in watchers {
        let _ = watcher.join();
    }
}

fn watch_volume(
    volume_roots: &[String],
    volume: &str,
    entries: &DashMap<String, FileEntry>,
    generation: u64,
    current_generation: &AtomicU64,
    catalog_dirty: &AtomicBool,
    start_usn: Option<i64>,
    expected_journal_id: Option<u64>,
    checkpoints: &Mutex<Vec<IndexJournal>>,
    persisted_checkpoints: &Mutex<Vec<IndexJournal>>,
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
    let mut parent_paths = HashMap::new();
    let mut pending_changes = Vec::new();
    let mut last_saved = Instant::now();
    {
        let mut checkpoints = checkpoints.lock();
        if !checkpoints
            .iter()
            .any(|checkpoint| checkpoint.volume.eq_ignore_ascii_case(volume))
        {
            checkpoints.push(IndexJournal {
                volume: volume.to_string(),
                journal_id,
                next_usn: cursor,
            });
        }
    }
    {
        let mut persisted = persisted_checkpoints.lock();
        if !persisted
            .iter()
            .any(|checkpoint| checkpoint.volume.eq_ignore_ascii_case(volume))
        {
            persisted.push(IndexJournal {
                volume: volume.to_string(),
                journal_id,
                next_usn: cursor,
            });
        }
    }

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
                    match apply_usn_record(
                        &buffer[offset..offset + length],
                        volume_roots,
                        volume,
                        entries,
                        &mut parent_paths,
                        Some(catalog_dirty),
                    ) {
                        Some(change) => pending_changes.push(change),
                        None => {}
                    }
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
                if last_saved.elapsed() >= Duration::from_secs(600) {
                    let mut persisted = persisted_checkpoints.lock();
                    let mut snapshot = persisted.clone();
                    if let Some(saved) = snapshot
                        .iter_mut()
                        .find(|checkpoint| checkpoint.volume.eq_ignore_ascii_case(volume))
                    {
                        saved.next_usn = cursor;
                    }
                    if crate::persistence::apply_index_changes(roots, &pending_changes, &snapshot)
                        .is_ok()
                    {
                        *persisted = snapshot;
                        pending_changes.clear();
                        last_saved = Instant::now();
                    }
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
        // A root without a volume journal (UNC share, POSIX path, ...) only
        // skips itself; aborting here would disable cache persistence for every
        // other root for the whole session.
        let Some(volume) = volume_root(root) else {
            continue;
        };
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
    let mut changes = Vec::new();
    for root in roots {
        let Some(volume) = volume_root(root) else {
            continue;
        };
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
                changes.push(IndexChange::DeleteTree(root.clone()));
                WalkDirSource::default().scan(std::slice::from_ref(root), &mut |batch| {
                    for entry in batch {
                        changes.push(IndexChange::Upsert(entry.clone()));
                        entries.insert(entry.path.clone(), entry);
                    }
                });
            }
            continue;
        };
        // A journal that cannot be queried right now only disables replay for
        // this root. Returning `None` here would discard the cached index and
        // force a full MFT rebuild on every launch.
        let Some((journal_id, first_usn, next_usn)) = query_journal(&volume) else {
            continue;
        };
        if journal_id != saved.journal_id || saved.next_usn < first_usn || saved.next_usn > next_usn
        {
            return None;
        }
        let (cursor, mut replayed) =
            replay_journal(root, &volume, saved.next_usn, next_usn, journal_id, entries)?;
        changes.append(&mut replayed);
        updated.push(IndexJournal {
            volume,
            journal_id,
            next_usn: cursor,
        });
    }
    // During startup restore, persist replayed changes without rewriting the
    // complete index. A first full scan writes its snapshot after this function.
    if scan_untracked_roots {
        // If replay cannot be committed together with its cursors, force a full
        // scan so the durable checkpoint can never skip unapplied changes.
        crate::persistence::apply_index_changes(roots, &changes, &updated).ok()?;
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
) -> Option<(i64, Vec<IndexChange>)> {
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
    let mut parent_paths = HashMap::new();
    let mut changes = Vec::new();
    let single_root = vec![root.to_string()];
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
                if let Some(change) = apply_usn_record(
                    &buffer[offset..offset + length],
                    &single_root,
                    volume,
                    entries,
                    &mut parent_paths,
                    None,
                ) {
                    changes.push(change);
                }
            }
            offset += length;
        }
        cursor = next;
    }
    unsafe { CloseHandle(handle) };
    (cursor >= target).then_some((cursor, changes))
}

fn apply_usn_record(
    record: &[u8],
    roots: &[String],
    volume: &str,
    entries: &DashMap<String, FileEntry>,
    parent_paths: &mut HashMap<u64, String>,
    catalog_dirty: Option<&AtomicBool>,
) -> Option<IndexChange> {
    let name_len = read_u16(record, 56) as usize;
    let name_offset = read_u16(record, 58) as usize;
    if name_len == 0 || name_offset + name_len > record.len() {
        return None;
    }
    let name = String::from_utf16_lossy(bytemuck_u16(&record[name_offset..name_offset + name_len]));
    let parent = read_u64(record, 16) & 0x0000_ffff_ffff_ffff;
    let parent_path = match parent_paths.get(&parent).cloned().or_else(|| {
        let path = path_for_file_id(volume, parent)?;
        if parent_paths.len() >= 65_536 {
            parent_paths.clear();
        }
        parent_paths.insert(parent, path.clone());
        Some(path)
    }) {
        Some(p) => p,
        None => return None,
    };
    let path = format!("{}\\{}", parent_path.trim_end_matches(['\\', '/']), name);
    if !roots.iter().any(|root| is_below_root(&path, root)) {
        return None;
    }
    let reason = read_u32(record, 40);
    if reason & (REASON_FILE_DELETE | REASON_RENAME_OLD_NAME) != 0 {
        parent_paths.clear();
        let prefix = format!("{}\\", path.trim_end_matches(['\\', '/']));
        entries.retain(|key, _| {
            !key.eq_ignore_ascii_case(&path)
                && !key
                    .get(..prefix.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(&prefix))
        });
        if let Some(dirty) = catalog_dirty {
            dirty.store(true, Ordering::Release);
        }
        return Some(IndexChange::DeleteTree(path));
    }
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        let entry = to_entry(Path::new(&path), &metadata);
        entries.insert(path, entry.clone());
        if let Some(dirty) = catalog_dirty {
            dirty.store(true, Ordering::Release);
        }
        return Some(IndexChange::Upsert(entry));
    }
    None
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

/// Resolve a file's logical size from the volume handle by file reference.
/// Mirrors [`path_for_file_id`] but only asks for attributes, which succeeds on
/// files whose data the user cannot read.
fn file_size_by_reference(volume_handle: isize, file_id: u64) -> Option<u64> {
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
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            0,
        )
    };
    if file == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut info = ByHandleFileInformation::default();
    let ok = unsafe { GetFileInformationByHandle(file, &mut info) } != 0;
    unsafe {
        CloseHandle(file);
    }
    ok.then_some(((info.file_size_high as u64) << 32) | info.file_size_low as u64)
}

#[derive(Default)]
pub(super) struct MftOrWalkSource;

impl IndexSource for MftOrWalkSource {
    fn scan(&self, roots: &[String], on_batch: &mut dyn FnMut(Vec<FileEntry>)) {
        let mut volumes = HashMap::<String, Vec<String>>::new();
        for root in roots {
            if let Some(volume) = volume_root(root) {
                volumes.entry(volume).or_default().push(root.clone());
            } else {
                WalkDirSource::default().scan(std::slice::from_ref(root), on_batch);
            }
        }
        for (_, volume_roots) in volumes {
            if !scan_ntfs_roots(&volume_roots, on_batch) {
                for root in &volume_roots {
                    WalkDirSource::default().scan(std::slice::from_ref(root), on_batch);
                }
            }
        }
    }
}

#[derive(Clone)]
struct Record {
    reference: u64,
    parent: u64,
    name: String,
    modified: i64,
    attributes: u32,
}

fn scan_ntfs_roots(roots: &[String], on_batch: &mut dyn FnMut(Vec<FileEntry>)) -> bool {
    let Some(root) = roots.first() else {
        return false;
    };
    let Some(volume) = volume_root(root) else {
        return false;
    };
    if !is_ntfs_volume(&volume) {
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
    let enumeration_started = Instant::now();
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
    let enumeration_elapsed = enumeration_started.elapsed();
    if !ok || records.is_empty() {
        unsafe {
            CloseHandle(handle);
        }
        return false;
    }

    // The volume handle stays open so sizes can be resolved by file reference
    // for the entries that actually enter the index.
    let path_build_started = Instant::now();
    let record_count = records.len();
    let mut batch = Vec::with_capacity(2000);
    let mut size_lookup = Duration::ZERO;
    let mut root_refs = Vec::new();
    let mut unresolved_roots = Vec::new();
    for root in roots {
        if let Some(reference) = file_reference(root) {
            root_refs.push((reference, root.trim_end_matches(['\\', '/']).to_string()));
        } else {
            unresolved_roots.push(root);
        }
    }
    if !root_refs.is_empty() {
        // MFT enumeration is volume-wide, but path construction is restricted to
        // descendants of the configured root instead of resolving every volume path.
        let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
        for record in records.values() {
            children
                .entry(record.parent)
                .or_default()
                .push(record.reference);
        }
        let mut visited = HashSet::new();
        for (root_id, root_path) in root_refs {
            let mut stack = vec![(root_id, root_path)];
            while let Some((parent_id, parent_path)) = stack.pop() {
                let Some(child_ids) = children.remove(&parent_id) else {
                    continue;
                };
                for child_id in child_ids {
                    if !visited.insert(child_id) {
                        continue;
                    }
                    let Some(record) = records.get(&child_id) else {
                        continue;
                    };
                    let path = format!("{}\\{}", parent_path, record.name);
                    if record.attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                        stack.push((child_id, path.clone()));
                    }
                    append_record(record, path, handle, &mut size_lookup, &mut batch, on_batch);
                }
            }
        }
        if !unresolved_roots.is_empty() {
            let mut paths = HashMap::new();
            for record in records.values() {
                let Some(path) = resolve_path(record.reference, &records, &mut paths, &volume, 0)
                else {
                    continue;
                };
                if unresolved_roots
                    .iter()
                    .any(|root| is_below_root(&path, root))
                {
                    append_record(record, path, handle, &mut size_lookup, &mut batch, on_batch);
                }
            }
        }
    } else {
        // Fall back when Windows cannot return the root's file reference.
        let mut paths = HashMap::new();
        for record in records.values() {
            let Some(path) = resolve_path(record.reference, &records, &mut paths, &volume, 0)
            else {
                continue;
            };
            if roots.iter().any(|root| is_below_root(&path, root)) {
                append_record(record, path, handle, &mut size_lookup, &mut batch, on_batch);
            }
        }
    }
    if !batch.is_empty() {
        on_batch(batch);
    }
    unsafe {
        CloseHandle(handle);
    }
    eprintln!(
        "[index-perf] mft_records={} enumerate_ms={} path_build_ms={} size_lookup_ms={}",
        record_count,
        enumeration_elapsed.as_millis(),
        path_build_started.elapsed().as_millis(),
        size_lookup.as_millis()
    );
    true
}

fn append_record(
    record: &Record,
    path: String,
    volume_handle: isize,
    size_lookup: &mut Duration,
    batch: &mut Vec<FileEntry>,
    on_batch: &mut dyn FnMut(Vec<FileEntry>),
) {
    let is_dir = record.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    // USN_RECORD carries no file size, so resolve it only for files that are
    // actually indexed. A failed lookup must not abort the scan.
    let size = if is_dir {
        0
    } else {
        let started = Instant::now();
        let size = file_size_by_reference(volume_handle, record.reference)
            .or_else(|| std::fs::metadata(&path).ok().map(|metadata| metadata.len()))
            .unwrap_or(0);
        *size_lookup += started.elapsed();
        size
    };
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
        size,
        modified: record.modified,
        ext,
        hidden: record.attributes & FILE_ATTRIBUTE_HIDDEN != 0,
        system: record.attributes & FILE_ATTRIBUTE_SYSTEM != 0,
    });
    if batch.len() == 2000 {
        on_batch(std::mem::take(batch));
        *batch = Vec::with_capacity(2000);
    }
}

fn file_reference(path: &str) -> Option<u64> {
    let wide: Vec<u16> = Path::new(path)
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut info = ByHandleFileInformation {
        attributes: 0,
        creation_time: [0; 2],
        last_access_time: [0; 2],
        last_write_time: [0; 2],
        volume_serial_number: 0,
        file_size_high: 0,
        file_size_low: 0,
        number_of_links: 0,
        file_index_high: 0,
        file_index_low: 0,
    };
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) } != 0;
    unsafe { CloseHandle(handle) };
    ok.then_some(
        (((info.file_index_high as u64) << 32) | info.file_index_low as u64)
            & 0x0000_ffff_ffff_ffff,
    )
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

/// 判断给定路径所在的卷是否为 NTFS。
///
/// 供热插磁盘的自动索引（见 `lib.rs::watch_new_volumes`）与 MFT 直读扫描共用，
/// 避免两处各写一份 `GetVolumeInformationW` 判定。
pub(crate) fn is_ntfs(root: &str) -> bool {
    volume_root(root).is_some_and(|volume| is_ntfs_volume(&volume))
}

/// 对已规范化的卷根（如 `D:\`）查询文件系统名，判断是否为 NTFS。
fn is_ntfs_volume(volume: &str) -> bool {
    let wide: Vec<u16> = volume.encode_utf16().chain(Some(0)).collect();
    let mut fs_name = [0u16; 32];
    unsafe {
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
    }
}

fn is_below_root(path: &str, root: &str) -> bool {
    let path = path.trim_end_matches(['\\', '/']);
    let root = root.trim_end_matches(['\\', '/']);
    path.len() > root.len()
        && path
            .get(..root.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(root))
        && path
            .as_bytes()
            .get(root.len())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    // These tests exercise Windows-only code paths; the module itself is only
    // compiled on Windows (see index_engine::mod), so `C:\`-style paths hold.
    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "stelix-mft-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn capture_journals_skips_roots_without_a_volume() {
        let roots = vec!["\\\\server\\share".to_string(), "/mnt/data".to_string()];
        let checkpoints = capture_journals(&roots);
        assert_eq!(
            checkpoints.as_ref().map(|items| items.len()),
            Some(0),
            "非盘符根应被跳过，而不是让整个捕获返回 None"
        );
    }

    #[test]
    fn resume_journals_skips_roots_without_a_volume() {
        let roots = vec!["\\\\server\\share".to_string(), "relative\\dir".to_string()];
        let entries = DashMap::new();
        let updated = resume_journals(&roots, &[], &entries, false);
        assert_eq!(
            updated.as_ref().map(|items| items.len()),
            Some(0),
            "非盘符根应被跳过，而不是让整个恢复返回 None"
        );
    }

    #[test]
    fn append_record_reads_size_from_filesystem_fallback() {
        let base = temp_path("size");
        fs::create_dir_all(&base).unwrap();
        let file = base.join("data.bin");
        fs::write(&file, vec![7u8; 4321]).unwrap();

        let record = Record {
            reference: 42,
            parent: 5,
            name: "data.bin".to_string(),
            modified: 0,
            attributes: 0,
        };
        let mut batch = Vec::new();
        let mut size_lookup = Duration::ZERO;
        append_record(
            &record,
            file.to_string_lossy().to_string(),
            INVALID_HANDLE_VALUE,
            &mut size_lookup,
            &mut batch,
            &mut |_batch: Vec<FileEntry>| {},
        );

        assert_eq!(batch.len(), 1);
        assert!(!batch[0].is_dir);
        assert_eq!(batch[0].size, 4321, "文件大小应回填为真实大小");

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn append_record_keeps_directory_size_zero() {
        let base = temp_path("dir-size");
        fs::create_dir_all(&base).unwrap();

        let record = Record {
            reference: 43,
            parent: 5,
            name: "dir-size".to_string(),
            modified: 0,
            attributes: FILE_ATTRIBUTE_DIRECTORY,
        };
        let mut batch = Vec::new();
        let mut size_lookup = Duration::ZERO;
        append_record(
            &record,
            base.to_string_lossy().to_string(),
            INVALID_HANDLE_VALUE,
            &mut size_lookup,
            &mut batch,
            &mut |_batch: Vec<FileEntry>| {},
        );

        assert_eq!(batch.len(), 1);
        assert!(batch[0].is_dir);
        assert_eq!(batch[0].size, 0, "目录大小固定为 0");

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn append_record_uses_zero_when_size_is_unavailable() {
        let path = temp_path("missing");
        let record = Record {
            reference: 44,
            parent: 5,
            name: "missing.bin".to_string(),
            modified: 0,
            attributes: 0,
        };
        let mut batch = Vec::new();
        let mut size_lookup = Duration::ZERO;
        append_record(
            &record,
            path.to_string_lossy().to_string(),
            INVALID_HANDLE_VALUE,
            &mut size_lookup,
            &mut batch,
            &mut |_batch: Vec<FileEntry>| {},
        );

        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].size, 0, "查询失败时回退为 0，不影响整次扫描");
    }
}
